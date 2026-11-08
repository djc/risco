//! A client for the local TCP protocol of Risco alarm panels
//!
//! This talks directly to the panel over the LAN (port 1000 by default), using the same protocol
//! as Risco's configuration software. It does not use Risco Cloud.
//!
//! Risco does not publish documentation for this protocol. This implementation is based on
//! [pyrisco](https://github.com/OnFreund/pyrisco) and
//! [risco-lan-bridge](https://github.com/TJForc/risco-lan-bridge).
//!
//! ```no_run
//! # async fn example() -> Result<(), risco::Error> {
//! let (client, mut events) = risco::Client::connect("192.168.1.10:1000", "5678", risco::Options::default()).await?;
//! println!("connected to {}", client.panel().model);
//!
//! for zone in client.zones().await? {
//!     println!("zone {} ({}): open = {}", zone.id, zone.label, zone.status.is_open());
//! }
//!
//! client.arm(1).await?;
//! while let Some(event) = events.recv().await {
//!     println!("{event:?}");
//! }
//! # Ok(())
//! # }
//! ```

mod client;
pub use client::{Client, Events, Options};
mod codec;
mod connection;
mod error;
pub use error::{Error, PanelError};
#[cfg(any(test, feature = "test-util"))]
#[doc(hidden)]
pub mod mock;
mod panel;
pub use panel::{Model, Panel};
mod types;
pub use types::{Event, Group, Partition, PartitionStatus, System, SystemStatus, Zone, ZoneStatus};

#[cfg(test)]
mod tests;
