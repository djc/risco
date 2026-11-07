//! End-to-end tests against a mock panel

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::codec::{self, Cipher, Deframer};
use crate::{Client, Error, Event, Group, Model, Options};

#[tokio::test]
async fn session() {
    let (addr, panel) = MockPanel::start(lightsys).await;
    let (client, mut events) = Client::connect(addr, "5678", options()).await.unwrap();

    let info = client.panel();
    assert_eq!(info.model, Model::LightSys);
    assert_eq!(info.firmware.as_deref(), Some("3.47.02"));
    assert_eq!(info.serial, "1234567890");
    assert_eq!(info.max_zones, 50);

    let zones = client.zones().await.unwrap();
    assert_eq!(zones.iter().map(|z| z.id).collect::<Vec<_>>(), [1, 2]);
    let front = &zones[0];
    assert_eq!(front.label, "Front door");
    assert_eq!(front.zone_type, 5);
    assert_eq!(front.technology.as_deref(), Some("W"));
    assert_eq!(front.partitions, [1]);
    assert_eq!(front.groups, [Group::A]);
    assert!(front.status.is_open() && !front.status.is_bypassed());
    let hallway = &zones[1];
    assert_eq!(hallway.partitions, [1, 2]);
    assert_eq!(hallway.groups, [Group::A, Group::C]);
    assert!(hallway.status.is_bypassed());

    let partitions = client.partitions().await.unwrap();
    assert_eq!(partitions.iter().map(|p| p.id).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(partitions[0].label, "House");
    assert!(partitions[0].status.is_ready() && partitions[0].status.is_disarmed());

    let system = client.system().await.unwrap();
    assert_eq!(system.label, "Home");
    assert!(system.status.has_ac_trouble());

    client.arm(1).await.unwrap();
    match events.recv().await {
        Some(Event::Partition { id: 1, status }) => assert!(status.is_armed()),
        event => panic!("unexpected event: {event:?}"),
    }

    match client.arm(2).await {
        Err(Error::Panel(error)) => assert_eq!(error.code(), "N12"),
        result => panic!("unexpected result: {result:?}"),
    }

    // Zone 2 is already bypassed, so only zone 1 should be toggled
    client.bypass_zone(2, true).await.unwrap();
    client.bypass_zone(1, true).await.unwrap();

    let time = jiff::civil::date(2026, 11, 5).at(13, 7, 0, 0);
    client.set_time(time).await.unwrap();

    client.disconnect().await;
    assert!(events.recv().await.is_none());

    let panel = panel.await.unwrap();
    // The unsolicited partition status was acknowledged
    assert_eq!(panel.acks, [50]);
    // The client mirrors the panel's encryption, which starts after LCL
    assert_eq!(panel.log[0], ("RID".to_owned(), false));
    assert_eq!(panel.log.last().unwrap(), &("DCN".to_owned(), true));
    assert!(!panel.received("ZBYPAS=2"));
    assert!(panel.received("ZBYPAS=1"));
}

#[tokio::test]
async fn panel_still_encrypting() {
    let (addr, panel) = MockPanel::start_with(true, lightsys).await;
    let (client, _events) = Client::connect(addr, "5678", options()).await.unwrap();
    assert_eq!(client.panel().model, Model::LightSys);
    assert!(client.zone_status(1).await.unwrap().is_open());
    client.disconnect().await;

    // The client switched to encryption as soon as it recovered the key
    let panel = panel.await.unwrap();
    assert_eq!(panel.log[0], ("RID".to_owned(), false));
    assert!(panel.log[1..].iter().all(|(_, encrypted)| *encrypted));
}

#[tokio::test]
async fn reset_stale_session() {
    // A session left encrypting from before ignores the handshake and only pushes status. Connect
    // should end it with a disconnect and report it as stale, rather than hanging or retrying.
    let (addr, panel) = MockPanel::start_with(true, |command| match command {
        "RID" => vec![Out::Push("PSTT1=E-R-----".to_owned())],
        _ => Vec::new(),
    })
    .await;

    match Client::connect(addr, "5678", options()).await {
        Err(Error::StaleSession) => {}
        result => panic!("expected a stale session, got {result:?}"),
    }

    let panel = panel.await.unwrap();
    assert!(
        panel.received("DCN"),
        "the stale session should get a disconnect"
    );
}

#[tokio::test]
async fn wrong_access_code() {
    let (addr, _panel) = MockPanel::start(lightsys).await;
    match Client::connect(addr, "0000", options()).await {
        Err(Error::Unauthorized) => {}
        result => panic!("unexpected result: {result:?}"),
    }
}

#[tokio::test]
async fn command_timeout() {
    let (addr, _panel) = MockPanel::start(|command| match command {
        "ZSTT*9?" => Vec::new(),
        _ => lightsys(command),
    })
    .await;

    let options = Options {
        timeout: Duration::from_millis(100),
        ..options()
    };
    let (client, _events) = Client::connect(addr, "5678", options).await.unwrap();
    assert!(matches!(client.zone_status(9).await, Err(Error::Timeout)));
    // The connection is still usable
    assert!(client.zone_status(1).await.unwrap().is_open());
}

#[tokio::test]
async fn skip_cancelled_commands() {
    let (addr, panel) = MockPanel::start(|command| match command {
        "ZSTT*9?" => Vec::new(),
        _ => lightsys(command),
    })
    .await;

    let options = Options {
        timeout: Duration::from_millis(200),
        concurrency: 1,
        ..options()
    };
    let (client, _events) = Client::connect(addr, "5678", options).await.unwrap();

    // Occupy the only slot, so the next command stays queued until we give up on it
    let slow = tokio::spawn({
        let client = client.clone();
        async move { client.zone_status(9).await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let queued = tokio::time::timeout(Duration::from_millis(50), client.zone_status(1)).await;
    assert!(queued.is_err());

    client.disconnect().await;
    assert!(matches!(slow.await.unwrap(), Err(Error::Timeout)));
    let panel = panel.await.unwrap();
    assert!(panel.received("ZSTT*9?"));
    assert!(!panel.received("ZSTT*1?"));
}

#[tokio::test]
async fn keepalive_timeout() {
    let (addr, _panel) = MockPanel::start(|command| match command {
        "CLOCK" => Vec::new(),
        _ => lightsys(command),
    })
    .await;

    let options = Options {
        timeout: Duration::from_millis(100),
        keepalive: Duration::from_millis(50),
        ..options()
    };
    let (client, mut events) = Client::connect(addr, "5678", options).await.unwrap();

    match events.recv().await {
        Some(Event::Disconnected(Error::Timeout)) => {}
        event => panic!("unexpected event: {event:?}"),
    }
    assert!(events.recv().await.is_none());
    assert!(matches!(
        client.zone_status(1).await,
        Err(Error::Disconnected)
    ));
}

/// Options for connecting to the mock panel, which needs no connect delay
fn options() -> Options {
    Options {
        start_delay: Duration::ZERO,
        ..Options::default()
    }
}

/// Simulates a LightSYS panel with two zones and two partitions in use
fn lightsys(command: &str) -> Vec<Out> {
    let id = |prefix: &str| {
        command
            .strip_prefix(prefix)?
            .strip_suffix('?')?
            .parse::<u16>()
            .ok()
    };

    let reply = match command {
        "RID" => "RID=1234".to_owned(),
        "LCL" | "RMT=5678" | "DCN" | "DISARM=1" | "ZBYPAS=1" => "ACK".to_owned(),
        "CLOCK=05/11/2026 13:07" => "ACK".to_owned(),
        "PNLCNF" => "PNLCNF=RP432".to_owned(),
        "FSVER?" => "FSVER=3.47.02 01/01/2025".to_owned(),
        "PNLSERD" => "PNLSERD=1234567890".to_owned(),
        "CLOCK" => "CLOCK=05/11/2026 13:07".to_owned(),
        "SYSLBL?" => "SYSLBL=Home            ".to_owned(),
        "SSTT?" => "SSTT=-A--------".to_owned(),
        "ARM=1" => {
            return vec![
                Out::Reply("ACK".to_owned()),
                Out::Push("PSTT1=EA------".to_owned()),
            ];
        }
        "ARM=2" => "N12".to_owned(),
        _ => {
            if let Some(id) = id("ZTYPE*") {
                let zone_type = match id {
                    1 => 5,
                    2..=4 => 1,
                    _ => 0,
                };
                format!("ZTYPE*{id}={zone_type}")
            } else if let Some(id) = id("ZLNKTYP") {
                let technology = match id {
                    1 => "W",
                    3 => "N",
                    _ => "E",
                };
                format!("ZLNKTYP{id}={technology}")
            } else if let Some(id) = id("ZSTT*") {
                let status = match id {
                    1 => "--O-------",
                    2 => "-------Y--",
                    _ => "---------N",
                };
                format!("ZSTT*{id}={status}")
            } else if let Some(id @ 1..=2) = id("ZLBL*") {
                let label = ["Front door      ", "Hallway         "][usize::from(id - 1)];
                format!("ZLBL*{id}={label}")
            } else if let Some(id @ 1..=2) = id("ZPART&*") {
                format!("ZPART&*{id}={}", id * 2 - 1)
            } else if let Some(id @ 1..=2) = id("ZAREA&*") {
                format!("ZAREA&*{id}={}", id * 4 - 3)
            } else if let Some(id) = id("PSTT") {
                let status = match id {
                    1 => "E-R-----",
                    2 => "E-------",
                    _ => "--------",
                };
                format!("PSTT{id}={status}")
            } else if let Some(id @ 1..=2) = id("PLBL") {
                let label = ["House           ", "Garage          "][usize::from(id - 1)];
                format!("PLBL{id}={label}")
            } else {
                "N05".to_owned()
            }
        }
    };

    vec![Out::Reply(reply)]
}

enum Out {
    /// Reply to the command
    Reply(String),
    /// Send an unsolicited message
    Push(String),
}

struct MockPanel {
    stream: TcpStream,
    deframer: Deframer,
    cipher: Cipher,
    encrypted: bool,
    next_push: u8,
    /// Commands received, and whether they were encrypted
    log: Vec<(String, bool)>,
    /// IDs of acknowledged unsolicited messages
    acks: Vec<u8>,
}

impl MockPanel {
    async fn start(
        handler: impl FnMut(&str) -> Vec<Out> + Send + 'static,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<Self>) {
        Self::start_with(false, handler).await
    }

    /// Start a mock panel, which may already be encrypting from an earlier connection
    async fn start_with(
        encrypted: bool,
        handler: impl FnMut(&str) -> Vec<Out> + Send + 'static,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<Self>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        (
            addr,
            tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                Self::new(stream, encrypted).serve(handler).await
            }),
        )
    }

    /// Wrap an accepted connection, possibly one that is still encrypting from an earlier session
    fn new(stream: TcpStream, encrypted: bool) -> Self {
        Self {
            stream,
            deframer: Deframer::default(),
            cipher: Cipher::new(0x1234),
            encrypted,
            next_push: 50,
            log: Vec::new(),
            acks: Vec::new(),
        }
    }

    async fn serve(mut self, mut handler: impl FnMut(&str) -> Vec<Out>) -> Self {
        while let Some((id, command, encrypted)) = self.recv().await {
            if command == "ACK" {
                self.acks.push(id);
                continue;
            }

            self.log.push((command.clone(), encrypted));
            for out in handler(&command) {
                match out {
                    Out::Reply(reply) => self.send(id, &reply).await,
                    Out::Push(message) => {
                        let id = self.next_push;
                        self.next_push += 1;
                        self.send(id, &message).await;
                    }
                }
            }

            match command.as_str() {
                "LCL" => self.encrypted = true,
                "DCN" => break,
                _ => {}
            }
        }

        self
    }

    async fn recv(&mut self) -> Option<(u8, String, bool)> {
        loop {
            if let Some(payload) = self.deframer.next_frame() {
                let frame = codec::decode(&payload, &self.cipher).unwrap();
                assert!(frame.crc_ok);
                return Some((frame.id.unwrap(), frame.body, frame.encrypted));
            }

            let mut buf = [0; 512];
            match self.stream.read(&mut buf).await.unwrap_or(0) {
                0 => return None,
                n => self.deframer.extend(&buf[..n]),
            }
        }
    }

    async fn send(&mut self, id: u8, body: &str) {
        let frame = codec::encode(id, body, self.encrypted.then_some(&self.cipher)).unwrap();
        // The client may already have closed the connection
        let _ = self.stream.write_all(&frame).await;
    }

    fn received(&self, command: &str) -> bool {
        self.log.iter().any(|(c, _)| c == command)
    }
}
