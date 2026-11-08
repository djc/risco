//! A mock panel, for testing code that talks to a panel
//!
//! This is only available with the `test-util` feature. It is not covered by semver guarantees.

use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::codec::{self, Cipher, Deframer};

/// Simulates a LightSYS panel with two zones and two partitions in use
pub fn lightsys(command: &str) -> Vec<Out> {
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

/// What the mock panel sends in response to a command
pub enum Out {
    /// Reply to the command
    Reply(String),
    /// Send an unsolicited message
    Push(String),
}

/// A panel that answers commands with a handler, over a single connection
pub struct MockPanel {
    stream: TcpStream,
    deframer: Deframer,
    cipher: Cipher,
    encrypted: bool,
    next_push: u8,
    /// Commands received, and whether they were encrypted
    pub log: Vec<(String, bool)>,
    /// IDs of acknowledged unsolicited messages
    pub acks: Vec<u8>,
}

impl MockPanel {
    /// Start a mock panel on a local port, returning its address and the task serving it
    pub async fn start(
        handler: impl FnMut(&str) -> Vec<Out> + Send + 'static,
    ) -> (SocketAddr, JoinHandle<Self>) {
        Self::start_with(false, handler).await
    }

    /// Start a mock panel, which may already be encrypting from an earlier connection
    pub async fn start_with(
        encrypted: bool,
        handler: impl FnMut(&str) -> Vec<Out> + Send + 'static,
    ) -> (SocketAddr, JoinHandle<Self>) {
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
    pub fn new(stream: TcpStream, encrypted: bool) -> Self {
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

    /// Answer commands with `handler`, until the connection is closed or a `DCN` is received
    pub async fn serve(mut self, mut handler: impl FnMut(&str) -> Vec<Out>) -> Self {
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

    /// Whether the given command was received
    pub fn received(&self, command: &str) -> bool {
        self.log.iter().any(|(c, _)| c == command)
    }
}
