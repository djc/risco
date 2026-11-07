//! Connect to a panel and print what it reports
//!
//! ```text
//! cargo run --example query -- <host[:port]> [access code]
//! RUST_LOG=risco=trace cargo run --example query -- 192.168.1.10
//! ```
//!
//! The port defaults to 1000 and the access code to 5678. This only reads from the panel; it
//! does not arm, disarm or bypass anything.
//!
//! Pressing Ctrl-C stops querying but still closes the connection properly, because the panel
//! can take a while to accept a new connection after an unclean disconnect. Press Ctrl-C again
//! to exit immediately.

use std::env;
use std::pin::pin;
use std::process::ExitCode;
use std::time::Instant;

use risco::{Client, Error, Events, Options};
use tokio::signal;
use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let mut args = env::args().skip(1);
    let Some(mut addr) = args.next() else {
        eprintln!("usage: query <host[:port]> [access code]");
        return ExitCode::FAILURE;
    };
    if !addr.contains(':') {
        addr.push_str(":1000");
    }
    let access_code = args.next().unwrap_or_else(|| "5678".to_owned());

    println!("Connecting to {addr}...");
    let start = Instant::now();
    let mut connect = pin!(Client::connect(
        addr.as_str(),
        &access_code,
        Options::default()
    ));

    let mut interrupted = false;
    let result = tokio::select! {
        result = &mut connect => result,
        _ = signal::ctrl_c() => {
            // Dropping the attempt would leave the panel with a half-open session
            eprintln!("Interrupted, waiting for the connection attempt to finish (Ctrl-C again to quit)");
            interrupted = true;
            match interruptible(connect).await {
                Some(result) => result,
                None => return ExitCode::from(INTERRUPTED),
            }
        }
    };

    let (client, events) = match result {
        Ok(connected) => connected,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };

    println!("Connected in {:.1?}", start.elapsed());
    let result = if interrupted {
        None
    } else {
        interruptible(query(&client)).await
    };
    if result.is_none() {
        eprintln!("Interrupted, disconnecting (Ctrl-C again to quit)");
    }

    disconnect(client, events).await;
    match result {
        Some(Ok(())) => ExitCode::SUCCESS,
        Some(Err(error)) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
        None => ExitCode::from(INTERRUPTED),
    }
}

async fn query(client: &Client) -> Result<(), Error> {
    let panel = client.panel();
    println!(
        "Panel: {} ({}), firmware {}, serial {}",
        panel.model,
        panel.panel_type,
        panel.firmware.as_deref().unwrap_or("unknown"),
        panel.serial,
    );
    println!(
        "Supports {} zones, {} partitions, {} outputs",
        panel.max_zones, panel.max_partitions, panel.max_outputs,
    );

    let system = client.system().await?;
    let status = &system.status;
    println!("\nSystem: {} [{}]", system.label, status.as_str());
    println!(
        "  {}",
        flags(&[
            (status.has_ac_trouble(), "AC trouble"),
            (status.has_low_battery(), "low battery"),
            (status.has_phone_line_trouble(), "phone line trouble"),
            (status.has_clock_trouble(), "clock trouble"),
            (status.is_box_tampered(), "box tampered"),
            (status.has_jamming(), "jamming"),
            (status.in_programming_mode(), "programming mode"),
        ])
    );

    let partitions = client.partitions().await?;
    println!("\nPartitions ({} in use):", partitions.len());
    for partition in &partitions {
        let status = &partition.status;
        println!(
            "  {:>3} {:<16} [{}] {}",
            partition.id,
            partition.label,
            status.as_str(),
            flags(&[
                (status.is_armed(), "armed"),
                (status.is_partially_armed(), "partially armed"),
                (status.is_disarmed(), "disarmed"),
                (status.is_ready(), "ready"),
                (status.is_alarmed(), "ALARM"),
                (status.has_trouble(), "trouble"),
            ]),
        );
    }

    let start = Instant::now();
    let zones = client.zones().await?;
    println!(
        "\nZones ({} in use, queried in {:.1?}):",
        zones.len(),
        start.elapsed()
    );
    for zone in &zones {
        let status = &zone.status;
        let partitions = zone
            .partitions
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>();
        let groups = zone
            .groups
            .iter()
            .map(|g| g.to_string())
            .collect::<Vec<_>>();
        println!(
            "  {:>3} {:<16} type {:>2}, tech {}, partitions [{}], groups [{}], [{}] {}",
            zone.id,
            zone.label,
            zone.zone_type,
            zone.technology.as_deref().unwrap_or("-"),
            partitions.join(","),
            groups.join(","),
            status.as_str(),
            flags(&[
                (status.is_open(), "open"),
                (status.is_armed(), "armed"),
                (status.is_bypassed(), "bypassed"),
                (status.is_alarmed(), "ALARM"),
                (status.is_tampered(), "tampered"),
                (status.has_low_battery(), "low battery"),
                (status.has_trouble(), "trouble"),
                (status.is_lost(), "lost"),
            ]),
        );
    }

    Ok(())
}

/// Close the connection, then print the events received during the session
async fn disconnect(client: Client, mut events: Events) {
    if interruptible(client.disconnect()).await.is_none() {
        return;
    }

    // The connection is closed, so this only drains messages the panel sent during the session
    let mut received = Vec::new();
    while let Some(event) = events.recv().await {
        received.push(event);
    }
    if !received.is_empty() {
        println!("\nEvents received during the session:");
        for event in received {
            println!("  {event:?}");
        }
    }
}

/// Run `future` to completion, unless Ctrl-C is pressed first
async fn interruptible<T>(future: impl Future<Output = T>) -> Option<T> {
    tokio::select! {
        output = future => Some(output),
        _ = signal::ctrl_c() => None,
    }
}

/// List the names of the flags that are set
fn flags(flags: &[(bool, &str)]) -> String {
    let set = flags
        .iter()
        .filter(|(set, _)| *set)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>();
    if set.is_empty() {
        "-".to_owned()
    } else {
        set.join(", ")
    }
}

/// The conventional exit code after an interrupt (128 + SIGINT)
const INTERRUPTED: u8 = 130;
