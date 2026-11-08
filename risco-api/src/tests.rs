//! End-to-end tests of the server and client, against a mock panel

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::CONTENT_TYPE;
use risco::Options;
use risco::mock::{MockPanel, Out, lightsys};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::client::{Client, Error};
use crate::server::Server;
use crate::{ArmMode, ArmRequest, Arming, DisarmRequest, Status};

#[tokio::test]
async fn arm_and_disarm() {
    // Partition 1 can be armed and disarmed, while partition 2 is never ready
    let mut partition = "E-R-----";
    let (addr, panel) = MockPanel::start(move |command| {
        partition = match command {
            "ARM=1" => "EA------",
            "STAY=1" => "E--H----",
            "DISARM=1" => "E-R-----",
            "PSTT1?" => return vec![Out::Reply(format!("PSTT1={partition}"))],
            _ => return lightsys(command),
        };
        vec![Out::Reply("ACK".to_owned())]
    })
    .await;

    let server = Server::start(&addr.to_string(), "5678", options());
    let (url, client) = serve(&server).await;
    let status = connected(&client).await;
    assert!(!status.armed);

    // The wire format, as documented
    let disarmed = concat!(
        r#"{"armed":false,"partitions":["#,
        r#"{"id":1,"label":"House","state":"disarmed","ready":true,"alarm":false},"#,
        r#"{"id":2,"label":"Garage","state":"disarmed","ready":false,"alarm":false}]}"#,
    );
    let http = reqwest::Client::new();
    let response = http.get(format!("{url}/status")).send().await.unwrap();
    assert_eq!(response.text().await.unwrap(), disarmed);

    // Arming an armed partition again is a no-op
    let request = ArmRequest {
        partition: Some(1),
        mode: ArmMode::Full,
    };
    for _ in 0..2 {
        let status = client.arm(request).await.unwrap();
        assert!(status.armed);
        assert_eq!(status.partitions[0].state, Arming::Armed);
    }

    // Without a partition, all partitions are armed
    match client.arm(ArmRequest::default()).await {
        Err(Error::Api { status, response }) if status == StatusCode::CONFLICT => {
            assert_eq!(response.error, "panel error N12 (system not ready to arm)");
            assert_eq!(response.code.as_deref(), Some("N12"));
        }
        result => panic!("unexpected result: {result:?}"),
    }

    let request = ArmRequest {
        partition: Some(1),
        mode: ArmMode::Partial,
    };
    let status = client.arm(request).await.unwrap();
    assert_eq!(status.partitions[0].state, Arming::PartiallyArmed);

    let request = DisarmRequest { partition: Some(7) };
    match client.disarm(request).await {
        Err(Error::Api { status, response }) if status == StatusCode::NOT_FOUND => {
            assert_eq!(response.error, "partition 7 is not in use");
            assert_eq!(response.code, None);
        }
        result => panic!("unexpected result: {result:?}"),
    }

    let response = http
        .post(format!("{url}/disarm"))
        .header(CONTENT_TYPE, "application/json")
        .body(r#"{"partiton":1}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.text().await.unwrap();
    assert!(body.contains("unknown field `partiton`"));

    // Without a body, all partitions are disarmed; partition 2 already is, so it is skipped
    let response = http.post(format!("{url}/disarm")).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text().await.unwrap(), disarmed);

    server.shutdown().await;
    match client.status().await {
        Err(Error::Api { status, response }) if status == StatusCode::SERVICE_UNAVAILABLE => {
            assert_eq!(response.error, "not connected to the panel");
        }
        result => panic!("unexpected result: {result:?}"),
    }

    let panel = panel.await.unwrap();
    let commands = |command| panel.log.iter().filter(|(c, _)| c == command).count();
    assert_eq!(commands("ARM=1"), 1);
    assert_eq!(commands("ARM=2"), 1);
    assert_eq!(commands("DISARM=2"), 0);
    assert_eq!(panel.log.last().unwrap().0, "DCN");
}

#[tokio::test]
async fn reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (reconnected_tx, reconnected) = oneshot::channel();
    let panel = tokio::spawn(async move {
        // The first connection stops answering keepalives
        let (stream, _) = listener.accept().await.unwrap();
        MockPanel::new(stream, false)
            .serve(|command| match command {
                "CLOCK" => Vec::new(),
                _ => lightsys(command),
            })
            .await;

        let (stream, _) = listener.accept().await.unwrap();
        reconnected_tx.send(()).unwrap();
        MockPanel::new(stream, false).serve(lightsys).await
    });

    let mut options = options();
    options.timeout = Duration::from_millis(100);
    options.keepalive = Duration::from_millis(50);
    let server = Server::start_with(
        &addr.to_string(),
        "5678",
        options,
        Duration::from_millis(10),
    );
    let (_, client) = serve(&server).await;

    reconnected.await.unwrap();
    assert!(!connected(&client).await.armed);

    server.shutdown().await;
    assert!(panel.await.unwrap().received("DCN"));
}

/// Serve the API on a local port, returning its URL and a client for it
async fn serve(server: &Server) -> (String, Client) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = server.router();
    tokio::spawn(async move { axum::serve(listener, router).await });
    let client = Client::new(&url);
    (url, client)
}

/// Wait for the server to connect to the panel, returning the status
async fn connected(client: &Client) -> Status {
    let poll = async {
        loop {
            match client.status().await {
                Ok(status) => return status,
                Err(Error::Api { status, .. }) if status == StatusCode::SERVICE_UNAVAILABLE => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => panic!("unexpected error: {error}"),
            }
        }
    };

    tokio::time::timeout(Duration::from_secs(5), poll)
        .await
        .unwrap()
}

/// Options for connecting to the mock panel, which needs no connect delay
fn options() -> Options {
    let mut options = Options::default();
    options.start_delay = Duration::ZERO;
    options
}
