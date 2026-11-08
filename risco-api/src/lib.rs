//! A JSON API over HTTP for Risco alarm panels
//!
//! The API reports whether the alarm is armed, and can arm and disarm it. This crate has the
//! JSON types for the API, and optionally:
//!
//! - a server, in the `server` module (with the `server` feature), which keeps a connection to
//!   the panel open using the [`risco`](https://docs.rs/risco) crate, and the `risco-api-server`
//!   binary that runs it
//! - a client, in the `client` module (with the `client` feature)
//!
//! # Endpoints
//!
//! - `GET /status` reports whether the alarm is armed
//! - `POST /arm` arms all partitions. An optional [`ArmRequest`] body selects a single partition
//!   and the mode, `full` (the default) or `partial`: `{"partition": 1, "mode": "partial"}`
//! - `POST /disarm` disarms all partitions, or only the one selected by an optional
//!   [`DisarmRequest`] body like `{"partition": 1}`
//!
//! Partitions that are already in the requested state are skipped. All endpoints respond with
//! the [`Status`], as it is after the command:
//!
//! ```json
//! {
//!   "armed": true,
//!   "partitions": [
//!     { "id": 1, "label": "House", "state": "armed", "ready": true, "alarm": false },
//!     { "id": 2, "label": "Garage", "state": "disarmed", "ready": true, "alarm": false }
//!   ]
//! }
//! ```
//!
//! Errors are reported as an [`ErrorResponse`] like `{"error": "..."}`, with status 503 while
//! there is no connection to the panel, 404 for a partition that is not in use, 409 when the panel
//! refuses a command (adding the panel's error `code`, such as `N12` when a partition is not ready
//! to arm) and 504 when the panel does not reply in time.
//!
//! **The API has no authentication**: anyone who can reach it can disarm the alarm. Only serve it
//! on a trusted network, or behind a proxy that authenticates requests.

use serde::{Deserialize, Serialize};

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "server")]
pub mod server;

#[cfg(all(test, feature = "client", feature = "server"))]
mod tests;

/// Whether the alarm is armed, as returned by all endpoints
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Status {
    /// Whether any partition is armed, fully or partially
    pub armed: bool,
    /// The partitions in use
    pub partitions: Vec<PartitionState>,
}

/// The state of a partition (an independently armed area)
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PartitionState {
    /// Partition ID, starting at 1
    pub id: u8,
    /// The partition's name
    pub label: String,
    /// Whether the partition is armed
    pub state: Arming,
    /// Whether the partition is ready to be armed
    pub ready: bool,
    /// Whether the partition is in alarm
    pub alarm: bool,
}

/// Whether a partition is armed
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Arming {
    Disarmed,
    /// Fully armed
    Armed,
    /// Partially armed, in stay or home mode
    PartiallyArmed,
}

// Unknown fields in requests are rejected, so a misspelled `partition` doesn't select all
// partitions

/// The body of a `POST /arm` request
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ArmRequest {
    /// The partition to arm, or `None` for all partitions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<u8>,
    pub mode: ArmMode,
}

/// How to arm a partition
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmMode {
    /// Arm fully
    #[default]
    Full,
    /// Arm partially, in stay or home mode
    Partial,
}

/// The body of a `POST /disarm` request
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisarmRequest {
    /// The partition to disarm, or `None` for all partitions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<u8>,
}

/// The body of an error response
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ErrorResponse {
    /// A description of the error
    pub error: String,
    /// The panel's error code, if the panel refused a command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}
