//! End-to-end tests against a mock panel

use std::time::Duration;

use crate::mock::{MockPanel, Out, lightsys};
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
