//! An HTTP server exposing a panel through a JSON API
//!
//! [`Server`] keeps a connection to the panel open in the background, reconnecting whenever it
//! fails. Its [router](Server::router) serves the endpoints described in the [crate
//! documentation](crate).
//!
//! ```no_run
//! # async fn example() -> std::io::Result<()> {
//! use risco_api::server::Server;
//!
//! let server = Server::start("192.168.1.10:1000", "5678", risco::Options::default());
//! let listener = tokio::net::TcpListener::bind("127.0.0.1:8080").await?;
//! axum::serve(listener, server.router())
//!     .with_graceful_shutdown(async {
//!         let _ = tokio::signal::ctrl_c().await;
//!     })
//!     .await?;
//!
//! // After an unclean disconnect, the panel can take a long time to accept a new connection
//! server.shutdown().await;
//! # Ok(())
//! # }
//! ```

use std::fmt;
use std::ops::ControlFlow;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use risco::{Client, Error, Event, Events, Options, PartitionStatus};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::{ArmMode, ArmRequest, Arming, DisarmRequest, ErrorResponse, PartitionState, Status};

/// How long to wait before reconnecting; doubled after each failed attempt
const RETRY_DELAY: Duration = Duration::from_secs(5);
/// The longest wait between connection attempts
const MAX_RETRY_DELAY: Duration = Duration::from_secs(300);

/// Keeps a connection to a panel open, and serves a JSON API for it
///
/// The connection is made in the background, and the API responds with 503 (Service
/// Unavailable) until it is established. When the connection fails, the server reconnects. It
/// waits longer after each failed attempt, up to five minutes, because panels can stop
/// responding for a while when reconnected to too eagerly.
#[derive(Debug)]
pub struct Server {
    connection: Shared,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl Server {
    /// Start connecting to a panel in the background
    ///
    /// The arguments are the same as for [`Client::connect()`]. This must be called from within
    /// a Tokio runtime.
    pub fn start(addr: &str, access_code: &str, options: Options) -> Self {
        Self::start_with(addr, access_code, options, RETRY_DELAY)
    }

    pub(crate) fn start_with(
        addr: &str,
        access_code: &str,
        options: Options,
        retry_delay: Duration,
    ) -> Self {
        let (state, connection) = watch::channel(None);
        let (shutdown, shutdown_rx) = oneshot::channel();
        let supervisor = Supervisor {
            addr: addr.to_owned(),
            access_code: access_code.to_owned(),
            options,
            state,
            retry_delay,
        };

        Self {
            connection,
            shutdown,
            task: tokio::spawn(supervisor.run(shutdown_rx)),
        }
    }

    /// A router serving the JSON API
    ///
    /// This can be nested in a larger application. Once the server is shut down or dropped, all
    /// requests fail with 503 (Service Unavailable).
    pub fn router(&self) -> Router {
        Router::new()
            .route("/status", get(status))
            .route("/arm", post(arm))
            .route("/disarm", post(disarm))
            .with_state(self.connection.clone())
    }

    /// Disconnect from the panel and stop reconnecting
    ///
    /// If a connection attempt is in progress, this waits for it to finish, so the panel is not
    /// left with a half-open session. Dropping the `Server` also disconnects, without waiting.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(());
        if let Err(error) = self.task.await
            && let Ok(panic) = error.try_into_panic()
        {
            std::panic::resume_unwind(panic);
        }
    }
}

/// The background task that maintains the connection
struct Supervisor {
    addr: String,
    access_code: String,
    options: Options,
    /// The current connection, published to the API handlers
    state: watch::Sender<Option<Connected>>,
    retry_delay: Duration,
}

impl Supervisor {
    /// Keep connecting to the panel until `shutdown` completes or its sender is dropped
    async fn run(self, mut shutdown: oneshot::Receiver<()>) {
        let mut delay = self.retry_delay;
        loop {
            // Dropping a connection attempt would leave the panel with a half-open session, so
            // let it finish even when shutting down
            let mut connect = pin!(Client::connect(
                self.addr.as_str(),
                &self.access_code,
                self.options.clone(),
            ));
            let (result, stop) = tokio::select! {
                result = &mut connect => (result, false),
                _ = &mut shutdown => (connect.await, true),
            };

            match result {
                Ok((client, _)) if stop => {
                    client.disconnect().await;
                    return;
                }
                Ok((client, events)) => {
                    if self.serve(client, events, &mut shutdown).await.is_break() {
                        return;
                    }
                    delay = self.retry_delay;
                }
                Err(_) if stop => return,
                Err(error) => warn!(%error, "failed to connect to panel"),
            }

            debug!(?delay, "waiting before reconnecting");
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                _ = &mut shutdown => return,
            }
            delay = (delay * 2).min(MAX_RETRY_DELAY);
        }
    }

    /// Publish a connection to the API handlers until it fails, or until shutting down
    async fn serve(
        &self,
        client: Client,
        mut events: Events,
        shutdown: &mut oneshot::Receiver<()>,
    ) -> ControlFlow<()> {
        // Labels can only be changed in programming mode, so they are queried once per connection
        let partitions = match client.partitions().await {
            Ok(partitions) => partitions,
            Err(error) => {
                warn!(%error, "failed to query partitions");
                client.disconnect().await;
                return ControlFlow::Continue(());
            }
        };

        info!(
            model = %client.panel().model,
            partitions = partitions.len(),
            "connected to panel"
        );
        self.state.send_replace(Some(Connected {
            client: client.clone(),
            partitions: partitions.into_iter().map(|p| (p.id, p.label)).collect(),
        }));

        let flow = tokio::select! {
            error = closed(&mut events) => {
                match error {
                    Some(error) => warn!(%error, "lost connection to panel"),
                    None => warn!("connection to panel closed"),
                }
                ControlFlow::Continue(())
            }
            _ = &mut *shutdown => ControlFlow::Break(()),
        };

        self.state.send_replace(None);
        if flow.is_break() {
            client.disconnect().await;
        }
        flow
    }
}

/// Wait for the connection to close, returning the error that closed it
async fn closed(events: &mut Events) -> Option<Error> {
    while let Some(event) = events.recv().await {
        match event {
            Event::Disconnected(error) => return Some(error),
            event => debug!(?event, "received event"),
        }
    }

    None
}

/// The current connection, shared with the API handlers
type Shared = watch::Receiver<Option<Connected>>;

#[derive(Clone, Debug)]
struct Connected {
    client: Client,
    /// IDs and labels of the partitions in use
    partitions: Arc<[(u8, String)]>,
}

impl Connected {
    /// Query the status of all partitions
    async fn status(&self) -> Result<Status, Error> {
        let mut partitions = Vec::with_capacity(self.partitions.len());
        for (id, label) in self.partitions.iter() {
            let status = self.client.partition_status(*id).await?;
            partitions.push(PartitionState {
                id: *id,
                label: label.clone(),
                state: arming(&status),
                ready: status.is_ready(),
                alarm: status.is_alarmed(),
            });
        }

        Ok(Status {
            armed: partitions.iter().any(|p| p.state != Arming::Disarmed),
            partitions,
        })
    }

    /// The IDs of the selected partition, or of all partitions if none is selected
    fn select(&self, partition: Option<u8>) -> Result<Vec<u8>, ApiError> {
        match partition {
            None => Ok(self.partitions.iter().map(|(id, _)| *id).collect()),
            Some(id) if self.partitions.iter().any(|(p, _)| *p == id) => Ok(vec![id]),
            Some(id) => Err(ApiError::NoPartition(id)),
        }
    }
}

async fn status(State(connection): State<Shared>) -> Result<Json<Status>, ApiError> {
    let connected = connected(&connection)?;
    Ok(Json(connected.status().await?))
}

async fn arm(
    State(connection): State<Shared>,
    request: Result<Option<Json<ArmRequest>>, JsonRejection>,
) -> Result<Json<Status>, ApiError> {
    let ArmRequest { partition, mode } = body(request)?;
    let connected = connected(&connection)?;
    let client = &connected.client;
    for id in connected.select(partition)? {
        let status = client.partition_status(id).await?;
        match mode {
            ArmMode::Full if !status.is_armed() => client.arm(id).await?,
            ArmMode::Partial if !status.is_partially_armed() => client.partial_arm(id).await?,
            _ => {}
        }
    }

    Ok(Json(connected.status().await?))
}

async fn disarm(
    State(connection): State<Shared>,
    request: Result<Option<Json<DisarmRequest>>, JsonRejection>,
) -> Result<Json<Status>, ApiError> {
    let DisarmRequest { partition } = body(request)?;
    let connected = connected(&connection)?;
    let client = &connected.client;
    for id in connected.select(partition)? {
        if !client.partition_status(id).await?.is_disarmed() {
            client.disarm(id).await?;
        }
    }

    Ok(Json(connected.status().await?))
}

fn connected(connection: &Shared) -> Result<Connected, ApiError> {
    connection.borrow().clone().ok_or(ApiError::NotConnected)
}

/// The request body, or the defaults if the request has none
fn body<T: Default>(request: Result<Option<Json<T>>, JsonRejection>) -> Result<T, ApiError> {
    Ok(request?.map(|Json(body)| body).unwrap_or_default())
}

fn arming(status: &PartitionStatus) -> Arming {
    if status.is_armed() {
        Arming::Armed
    } else if status.is_partially_armed() {
        Arming::PartiallyArmed
    } else {
        Arming::Disarmed
    }
}

/// An error, as reported to API clients
#[derive(Debug)]
enum ApiError {
    /// There is no connection to the panel at the moment
    NotConnected,
    /// The request selected a partition that is not in use
    NoPartition(u8),
    /// The request body could not be parsed
    Request(JsonRejection),
    Panel(Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::NotConnected | Self::Panel(Error::Disconnected) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::NoPartition(_) => StatusCode::NOT_FOUND,
            Self::Request(rejection) => rejection.status(),
            Self::Panel(Error::Panel(_)) => StatusCode::CONFLICT,
            Self::Panel(Error::Timeout) => StatusCode::GATEWAY_TIMEOUT,
            Self::Panel(_) => StatusCode::BAD_GATEWAY,
        };

        let code = match &self {
            Self::Panel(Error::Panel(error)) => Some(error.code().to_owned()),
            _ => None,
        };

        let body = ErrorResponse {
            error: self.to_string(),
            code,
        };
        (status, Json(body)).into_response()
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConnected => f.write_str("not connected to the panel"),
            Self::NoPartition(id) => write!(f, "partition {id} is not in use"),
            Self::Request(rejection) => f.write_str(&rejection.body_text()),
            Self::Panel(error) => error.fmt(f),
        }
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        Self::Request(rejection)
    }
}

impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        Self::Panel(error)
    }
}
