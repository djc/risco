use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpStream, ToSocketAddrs};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tracing::{debug, info};

use crate::connection::{Connection, Request};
use crate::panel::{self, Panel};
use crate::types::{
    self, Group, Partition, PartitionStatus, System, SystemStatus, Zone, ZoneStatus,
};
use crate::{Error, Event};

/// A connection to a panel
///
/// Cloning a `Client` is cheap; all clones share the same connection. The connection stays
/// open until [`Client::disconnect()`] is called, all clones are dropped, or it fails.
///
/// Zone and partition IDs start at 1.
#[derive(Clone, Debug)]
pub struct Client {
    requests: mpsc::Sender<Request>,
    panel: Arc<Panel>,
}

impl Client {
    /// Connect to a panel
    ///
    /// This takes at least as long as the [connect delay](Options::connect_delay), 10 seconds
    /// by default. After a connection is closed, the panel needs a few seconds before it accepts
    /// a new one.
    ///
    /// If an earlier connection was not closed cleanly, the panel can be left encrypting and
    /// will ignore a new connection's handshake. This is detected, the stale session is ended
    /// with a disconnect so the panel resets, and [`Error::StaleSession`] is returned; a fresh
    /// connection attempt should then succeed.
    pub async fn connect(
        addr: impl ToSocketAddrs,
        access_code: &str,
        options: Options,
    ) -> Result<(Self, Events), Error> {
        let stream = TcpStream::connect(addr).await?;
        if !options.start_delay.is_zero() {
            info!(delay = ?options.start_delay, "waiting for panel to become ready");
            tokio::time::sleep(options.start_delay).await;
        }

        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let mut conn = Connection::new(
            stream,
            events_tx,
            options.timeout,
            options.keepalive,
            options.concurrency,
        );

        conn.establish_cipher().await?;
        if conn.stale() {
            // A session left encrypting by an earlier unclean disconnect ignores the handshake;
            // end it so the panel resets, and let the caller reconnect.
            debug!("resetting stale encrypted session");
            let _ = conn.send_disconnect().await;
            return Err(Error::StaleSession);
        }

        let panel = Self::handshake(&mut conn, access_code).await?;
        debug!(?panel, "connected to panel");

        let (requests, requests_rx) = mpsc::channel(32);
        tokio::spawn(conn.run(requests_rx));
        let client = Self {
            requests,
            panel: Arc::new(panel),
        };

        Ok((client, Events(events_rx)))
    }

    /// Authenticate and read the panel's identity, after the cipher is established
    async fn handshake(conn: &mut Connection, access_code: &str) -> Result<Panel, Error> {
        expect_ack(conn.call("LCL").await?)?;
        match conn.call(&format!("RMT={access_code}")).await {
            Ok(reply) if reply == "ACK" => {}
            Ok(_) | Err(Error::Panel(_)) => return Err(Error::Unauthorized),
            Err(error) => return Err(error),
        }

        let panel_type = value(&conn.call("PNLCNF").await?)?.trim().to_owned();
        let firmware = if panel::is_legacy(&panel_type) {
            None
        } else {
            let reply = conn.call("FSVER?").await?;
            let firmware = value(&reply)?.split_whitespace().next().unwrap_or_default();
            Some(firmware.to_owned())
        };
        let serial = value(&conn.call("PNLSERD").await?)?.trim().to_owned();
        Panel::new(panel_type, firmware, serial)
    }

    /// Information about the panel, retrieved while connecting
    pub fn panel(&self) -> &Panel {
        &self.panel
    }

    /// Query all zones in use
    pub async fn zones(&self) -> Result<Vec<Zone>, Error> {
        let mut zones = self
            .discover(1..=self.panel.max_zones, |client, id| async move {
                client.zone(id).await
            })
            .await?;
        zones.sort_by_key(|zone| zone.id);
        Ok(zones)
    }

    /// Query a zone, returning `None` if it is not in use
    pub async fn zone(&self, id: u16) -> Result<Option<Zone>, Error> {
        match self.query_zone(id).await {
            Err(Error::Panel(_)) => Ok(None),
            result => result,
        }
    }

    async fn query_zone(&self, id: u16) -> Result<Option<Zone>, Error> {
        let zone_type = self.query(&format!("ZTYPE*{id}?")).await?;
        let zone_type = zone_type
            .trim()
            .parse()
            .map_err(|_| Error::UnexpectedResponse(zone_type))?;
        if zone_type == 0 {
            return Ok(None);
        }

        let legacy = self.panel.is_legacy();
        let technology = if legacy {
            None
        } else {
            match self.query(&format!("ZLNKTYP{id}?")).await?.trim() {
                "N" => return Ok(None),
                technology => Some(technology.to_owned()),
            }
        };

        let status = self.zone_status(id).await?;
        if status.as_str().ends_with('N') {
            return Ok(None);
        }

        let label = self.query(&format!("ZLBL*{id}?")).await?;
        let partitions = self.query(&format!("ZPART&*{id}?")).await?;
        let groups = if legacy {
            Vec::new()
        } else {
            Group::parse_mask(&self.query(&format!("ZAREA&*{id}?")).await?)
        };

        Ok(Some(Zone {
            id,
            label: label.trim().to_owned(),
            zone_type,
            technology,
            partitions: types::parse_partitions(&partitions),
            groups,
            status,
        }))
    }

    /// Query the current status of a zone
    pub async fn zone_status(&self, id: u16) -> Result<ZoneStatus, Error> {
        Ok(ZoneStatus::new(&self.query(&format!("ZSTT*{id}?")).await?))
    }

    /// Query all partitions in use
    pub async fn partitions(&self) -> Result<Vec<Partition>, Error> {
        let mut partitions = self
            .discover(1..=self.panel.max_partitions, |client, id| async move {
                client.partition(id).await
            })
            .await?;
        partitions.sort_by_key(|partition| partition.id);
        Ok(partitions)
    }

    /// Query a partition, returning `None` if it is not in use
    pub async fn partition(&self, id: u8) -> Result<Option<Partition>, Error> {
        let status = match self.partition_status(id).await {
            Ok(status) if status.exists() => status,
            Ok(_) | Err(Error::Panel(_)) => return Ok(None),
            Err(error) => return Err(error),
        };

        let label = match self.query(&format!("PLBL{id}?")).await {
            Ok(label) => label,
            Err(Error::Panel(_)) => return Ok(None),
            Err(error) => return Err(error),
        };

        Ok(Some(Partition {
            id,
            label: label.trim().to_owned(),
            status,
        }))
    }

    /// Query the current status of a partition
    pub async fn partition_status(&self, id: u8) -> Result<PartitionStatus, Error> {
        Ok(PartitionStatus::new(
            &self.query(&format!("PSTT{id}?")).await?,
        ))
    }

    /// Query the panel's name and status
    pub async fn system(&self) -> Result<System, Error> {
        let label = self.query("SYSLBL?").await?;
        Ok(System {
            label: label.trim().to_owned(),
            status: self.system_status().await?,
        })
    }

    /// Query the current system status
    pub async fn system_status(&self) -> Result<SystemStatus, Error> {
        Ok(SystemStatus::new(&self.query("SSTT?").await?))
    }

    /// Fully arm a partition
    pub async fn arm(&self, partition: u8) -> Result<(), Error> {
        self.execute(&format!("ARM={partition}")).await
    }

    /// Partially arm a partition (stay or home mode)
    pub async fn partial_arm(&self, partition: u8) -> Result<(), Error> {
        self.execute(&format!("STAY={partition}")).await
    }

    /// Arm a group within a partition
    pub async fn group_arm(&self, partition: u8, group: Group) -> Result<(), Error> {
        let group = group.number();
        self.execute(&format!("GARM*{group}={partition}")).await
    }

    /// Disarm a partition
    pub async fn disarm(&self, partition: u8) -> Result<(), Error> {
        self.execute(&format!("DISARM={partition}")).await
    }

    /// Bypass a zone, or remove its bypass
    ///
    /// The panel only supports toggling the bypass, so this queries the zone's status first.
    pub async fn bypass_zone(&self, zone: u16, bypass: bool) -> Result<(), Error> {
        if self.zone_status(zone).await?.is_bypassed() == bypass {
            return Ok(());
        }

        self.execute(&format!("ZBYPAS={zone}")).await
    }

    /// Set the panel's clock
    pub async fn set_time(&self, time: jiff::civil::DateTime) -> Result<(), Error> {
        let command = format!(
            "CLOCK={:02}/{:02}/{:04} {:02}:{:02}",
            time.day(),
            time.month(),
            time.year(),
            time.hour(),
            time.minute(),
        );
        self.execute(&command).await
    }

    /// Send a raw command and return the panel's reply
    ///
    /// Replies to queries generally look like `ZSTT*1=--O-------`, while other commands are
    /// answered with `ACK`. If the panel replies with an error code, [`Error::Panel`] is returned.
    pub async fn command(&self, command: &str) -> Result<String, Error> {
        let (reply, rx) = oneshot::channel();
        let request = Request::Command {
            command: command.to_owned(),
            reply,
        };

        self.requests
            .send(request)
            .await
            .map_err(|_| Error::Disconnected)?;
        rx.await.map_err(|_| Error::Disconnected)?
    }

    /// Close the connection
    ///
    /// This closes the connection for all clones of this `Client`.
    pub async fn disconnect(self) {
        let (tx, rx) = oneshot::channel();
        if self.requests.send(Request::Disconnect(tx)).await.is_ok() {
            let _ = rx.await;
        }
    }

    /// Send a query and return the value from the reply
    async fn query(&self, command: &str) -> Result<String, Error> {
        let reply = self.command(command).await?;
        Ok(value(&reply)?.to_owned())
    }

    /// Send a command that should be acknowledged
    async fn execute(&self, command: &str) -> Result<(), Error> {
        expect_ack(self.command(command).await?)
    }

    /// Query items concurrently, collecting the ones that exist
    async fn discover<I, T, F, Fut>(
        &self,
        ids: impl IntoIterator<Item = I>,
        query: F,
    ) -> Result<Vec<T>, Error>
    where
        F: Fn(Self, I) -> Fut,
        Fut: Future<Output = Result<Option<T>, Error>> + Send + 'static,
        T: Send + 'static,
    {
        // The connection limits how many commands are in flight
        let mut tasks = JoinSet::new();
        for id in ids {
            tasks.spawn(query(self.clone(), id));
        }

        let mut found = Vec::new();
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(Some(item))) => found.push(item),
                Ok(Ok(None)) => {}
                Ok(Err(error)) => return Err(error),
                Err(error) => std::panic::resume_unwind(error.into_panic()),
            }
        }

        Ok(found)
    }
}

/// Extract the value from a reply like `KEY=value`
fn value(reply: &str) -> Result<&str, Error> {
    match reply.split_once('=') {
        Some((_, value)) => Ok(value),
        None => Err(Error::UnexpectedResponse(reply.to_owned())),
    }
}

fn expect_ack(reply: String) -> Result<(), Error> {
    match reply.as_str() {
        "ACK" => Ok(()),
        _ => Err(Error::UnexpectedResponse(reply)),
    }
}

/// Unsolicited messages from the panel
///
/// Messages are buffered from the moment the connection is established, so status changes
/// that happen while zones and partitions are being queried are not lost. Messages are buffered
/// without limit, so keep receiving them (or drop this) for long-lived connections.
#[derive(Debug)]
pub struct Events(mpsc::UnboundedReceiver<Event>);

impl Events {
    /// Receive the next event, or `None` once the connection is closed
    pub async fn recv(&mut self) -> Option<Event> {
        self.0.recv().await
    }
}

/// Options for connecting to a panel
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct Options {
    pub timeout: Duration,
    pub keepalive: Duration,
    pub concurrency: usize,
    pub start_delay: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            keepalive: Duration::from_secs(5),
            concurrency: 2,
            start_delay: Duration::from_secs(10),
        }
    }
}
