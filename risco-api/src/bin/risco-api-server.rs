//! Serve a JSON API for a panel over HTTP
//!
//! ```text
//! risco-api-server <host[:port]> [access code] [listen address]
//! cargo run -p risco-api --features server -- <host[:port]> [access code] [listen address]
//! ```
//!
//! The port defaults to 1000, the access code to 5678 and the listen address to 127.0.0.1:8080.
//! Arguments that are left out are taken from the `RISCO_PANEL`, `RISCO_ACCESS_CODE` and
//! `RISCO_LISTEN` environment variables if those are set, which keeps the access code out of the
//! process list.
//!
//! See the `risco_api` crate for the API. Try it with the `client` example, or with curl:
//!
//! ```text
//! curl localhost:8080/status
//! curl -X POST localhost:8080/arm
//! curl --json '{"partition": 1, "mode": "partial"}' localhost:8080/arm
//! curl -X POST localhost:8080/disarm
//! ```
//!
//! The API has no authentication, so anyone who can reach the listen address can disarm the
//! alarm.
//!
//! Pressing Ctrl-C (or sending SIGINT) stops the server and closes the connection to the panel
//! properly, because the panel can take a while to accept a new connection after an unclean
//! disconnect. Press Ctrl-C again to exit immediately.

use std::env;
use std::process::ExitCode;

use risco::Options;
use risco_api::server::Server;
use tokio::net::TcpListener;
use tokio::signal;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

/// The conventional exit code after an interrupt (128 + SIGINT)
const INTERRUPTED: u8 = 130;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // Log connection changes unless configured otherwise
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let mut args = env::args().skip(1);
    let mut arg = |name: &str| args.next().or_else(|| env::var(name).ok());
    let Some(mut addr) = arg("RISCO_PANEL") else {
        eprintln!("usage: risco-api-server <host[:port]> [access code] [listen address]");
        eprintln!("(or set RISCO_PANEL, RISCO_ACCESS_CODE and RISCO_LISTEN)");
        return ExitCode::FAILURE;
    };
    if !addr.contains(':') {
        addr.push_str(":1000");
    }
    let access_code = arg("RISCO_ACCESS_CODE").unwrap_or_else(|| "5678".to_owned());
    let listen = arg("RISCO_LISTEN").unwrap_or_else(|| "127.0.0.1:8080".to_owned());

    // Bind before connecting, so a bad listen address doesn't disturb the panel
    let listener = match TcpListener::bind(&listen).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("error: failed to listen on {listen}: {error}");
            return ExitCode::FAILURE;
        }
    };

    println!("Listening on http://{listen}, connecting to {addr}");
    let server = Server::start(&addr, &access_code, Options::default());
    let result = axum::serve(listener, server.router())
        .with_graceful_shutdown(async {
            let _ = signal::ctrl_c().await;
        })
        .await;

    eprintln!("Disconnecting (Ctrl-C again to quit)");
    tokio::select! {
        () = server.shutdown() => {}
        _ = signal::ctrl_c() => return ExitCode::from(INTERRUPTED),
    }

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
