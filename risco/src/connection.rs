//! The task that owns the TCP connection to the panel
//!
//! Requests from [`Client`](crate::Client) handles arrive over a channel. Each request is sent
//! with a command ID between 1 and 49, and the panel echoes that ID in its reply. Messages the
//! panel sends on its own use IDs of 50 and up; they are acknowledged and turned into events.

use std::fmt;
use std::ops::ControlFlow;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, MissedTickBehavior, interval_at, sleep_until, timeout_at};
use tracing::{debug, trace, warn};

use crate::codec::{self, Cipher, Deframer, Frame, MAX_REQUEST_ID};
use crate::{Error, Event, PanelError};

pub(crate) struct Connection {
    reader: OwnedReadHalf,
    deframer: Deframer,
    writer: OwnedWriteHalf,
    cipher: Cipher,
    /// Whether `cipher` was derived from the panel ID
    key_known: bool,
    /// Whether to encrypt outgoing frames, mirroring the panel's most recent frame
    encrypted: bool,
    /// Whether the panel was found in a stale encrypted session (see [`Self::stale`])
    stale: bool,
    next_id: u8,
    in_flight: [Option<InFlight>; MAX_REQUEST_ID as usize],
    events: mpsc::UnboundedSender<Event>,
    timeout: Duration,
    keepalive: Duration,
    concurrency: usize,
    /// Set once a disconnect was requested; no new requests are accepted
    closing: bool,
    /// Notified after the connection is closed in response to a disconnect request
    disconnected: Option<oneshot::Sender<()>>,
}

impl Connection {
    pub(crate) fn new(
        stream: TcpStream,
        events: mpsc::UnboundedSender<Event>,
        timeout: Duration,
        keepalive: Duration,
        concurrency: usize,
    ) -> Self {
        let (reader, writer) = stream.into_split();
        Self {
            reader,
            deframer: Deframer::default(),
            writer,
            cipher: Cipher::new(0),
            key_known: false,
            encrypted: false,
            stale: false,
            next_id: 1,
            in_flight: [const { None }; MAX_REQUEST_ID as usize],
            events,
            timeout,
            keepalive,
            concurrency,
            closing: false,
            disconnected: None,
        }
    }

    pub(crate) fn set_panel_id(&mut self, panel_id: u16) {
        self.cipher = Cipher::new(panel_id);
        self.key_known = true;
    }

    /// Whether the panel was found in a stale encrypted session left by an earlier, unclean
    /// disconnect, in which it ignores the handshake (see [`Self::establish_cipher`])
    pub(crate) fn stale(&self) -> bool {
        self.stale
    }

    /// Send a `DCN` to end a stale session, so the panel resets and accepts a fresh connection
    ///
    /// Best effort: the frame is written but no reply is awaited, since a panel in this state
    /// often does not answer.
    pub(crate) async fn send_disconnect(&mut self) -> Result<(), Error> {
        let id = self.allocate_id();
        self.write(id, "DCN").await
    }

    /// Learn the panel ID and set up encryption, without spawning the connection task
    ///
    /// Normally the panel answers the plaintext `RID` command with its ID. But a panel left
    /// encrypting by an earlier, unclean disconnect ignores the plaintext command and only sends
    /// encrypted frames. The key is then recovered by brute force from the first such frame (in
    /// [`Self::dispatch`]), which is all `RID` would have told us. In that case the panel is also
    /// ignoring the rest of the handshake, so [`Self::stale`] is set for the caller to reset it.
    pub(crate) async fn establish_cipher(&mut self) -> Result<(), Error> {
        let (tx, mut rx) = oneshot::channel();
        let id = self.start("RID", Waiter::Command(tx)).await?;
        loop {
            // A reply to RID means the handshake is live, even if the panel encrypts
            match rx.try_recv() {
                Ok(result) => {
                    let reply = result?;
                    let value = reply.split_once('=').map(|(_, v)| v).unwrap_or(&reply);
                    let panel_id = u32::from_str_radix(value.trim(), 16)
                        .map_err(|_| Error::UnexpectedResponse(reply.clone()))?;
                    // Only the low 16 bits affect the keystream
                    self.set_panel_id(panel_id as u16);
                    return Ok(());
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => return Err(Error::Disconnected),
            }

            if self.key_known {
                // Recovered from an unsolicited frame while RID got no reply: a stale session
                self.in_flight[usize::from(id - 1)] = None;
                self.stale = true;
                return Ok(());
            }

            self.pump().await?;
        }
    }

    /// Send a command and wait for its reply, without spawning the connection task
    ///
    /// Used while setting up the connection.
    pub(crate) async fn call(&mut self, command: &str) -> Result<String, Error> {
        let (tx, mut rx) = oneshot::channel();
        self.start(command, Waiter::Command(tx)).await?;
        loop {
            match rx.try_recv() {
                Ok(result) => return result,
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => return Err(Error::Disconnected),
            }

            self.pump().await?;
        }
    }

    /// Read and dispatch one frame, or fail pending requests whose deadline has passed
    async fn pump(&mut self) -> Result<(), Error> {
        let Some(deadline) = self.next_deadline() else {
            return Err(Error::Disconnected);
        };

        // No disconnect is pending during setup, so the control flow can be ignored
        let _ = match timeout_at(deadline, self.read_frame()).await {
            Ok(payload) => self.dispatch(payload?).await?,
            Err(_) => self.expire()?,
        };
        Ok(())
    }

    pub(crate) async fn run(mut self, mut requests: mpsc::Receiver<Request>) {
        let result = self.serve(&mut requests).await;

        // Close the socket and fail outstanding requests before reporting the outcome
        let Self {
            reader,
            writer,
            in_flight,
            events,
            disconnected,
            ..
        } = self;
        drop((reader, writer, in_flight, requests));

        if let Err(error) = result {
            debug!(%error, "connection failed");
            let _ = events.send(Event::Disconnected(error));
        }

        if let Some(disconnected) = disconnected {
            let _ = disconnected.send(());
        }
    }

    async fn serve(&mut self, requests: &mut mpsc::Receiver<Request>) -> Result<(), Error> {
        let mut keepalive = interval_at(Instant::now() + self.keepalive, self.keepalive);
        keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            let accepting = !self.closing && self.in_flight_count() < self.concurrency;
            let deadline = self.next_deadline();
            let keepalive_due = !self.closing && !self.keepalive_in_flight();

            let flow = tokio::select! {
                payload = self.read_frame() => match payload {
                    Ok(payload) => self.dispatch(payload).await?,
                    // The panel may close the connection in response to DCN
                    Err(Error::Disconnected) if self.closing => ControlFlow::Break(()),
                    Err(error) => return Err(error),
                },
                request = requests.recv(), if accepting => match request {
                    // Skip commands whose caller stopped waiting while they were queued
                    Some(Request::Command { reply, .. }) if reply.is_closed() => {
                        ControlFlow::Continue(())
                    }
                    Some(Request::Command { command, reply }) => {
                        self.start(&command, Waiter::Command(reply)).await?;
                        ControlFlow::Continue(())
                    }
                    Some(Request::Disconnect(disconnected)) => {
                        self.disconnected = Some(disconnected);
                        self.close().await?;
                        ControlFlow::Continue(())
                    }
                    // All clients have been dropped
                    None => {
                        self.close().await?;
                        ControlFlow::Continue(())
                    }
                },
                () = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                    self.expire()?
                }
                _ = keepalive.tick(), if keepalive_due => {
                    self.start("CLOCK", Waiter::KeepAlive).await?;
                    ControlFlow::Continue(())
                }
            };

            if flow.is_break() {
                return Ok(());
            }
        }
    }

    async fn close(&mut self) -> Result<(), Error> {
        self.closing = true;
        self.start("DCN", Waiter::Disconnect).await?;
        Ok(())
    }

    /// Send a command, registering `waiter` to receive the reply
    ///
    /// Returns the command ID the request was sent with. Only I/O errors are returned; other
    /// errors are passed to the waiter.
    async fn start(&mut self, command: &str, waiter: Waiter) -> Result<u8, Error> {
        let id = self.allocate_id();
        let frame = match codec::encode(id, command, self.encrypted.then_some(&self.cipher)) {
            Ok(frame) => frame,
            Err(error) => {
                if let Waiter::Command(reply) = waiter {
                    let _ = reply.send(Err(error));
                }
                return Ok(id);
            }
        };

        let logged = if command.starts_with("RMT=") {
            "RMT=<redacted>"
        } else {
            command
        };
        trace!(
            id,
            command = logged,
            encrypted = self.encrypted,
            "sending command"
        );

        self.writer.write_all(&frame).await?;
        self.in_flight[usize::from(id - 1)] = Some(InFlight {
            waiter,
            deadline: Instant::now() + self.timeout,
        });
        Ok(id)
    }

    async fn dispatch(&mut self, payload: Vec<u8>) -> Result<ControlFlow<()>, Error> {
        let mut decoded = codec::decode(&payload, &self.cipher);
        if !self.key_known && !decoded.as_ref().is_ok_and(|frame| frame.crc_ok) {
            // The panel may still be encrypting from a previous connection
            if let Some(panel_id) = codec::find_panel_id(&payload) {
                debug!(panel_id, "recovered panel ID from encrypted frame");
                self.set_panel_id(panel_id);
                decoded = codec::decode(&payload, &self.cipher);
            }
        }

        let frame = match decoded {
            Ok(frame) => frame,
            Err(reason) => {
                warn!(reason, "discarding malformed frame");
                return Ok(ControlFlow::Continue(()));
            }
        };

        self.encrypted = frame.encrypted;
        trace!(
            id = frame.id,
            body = %frame.body,
            encrypted = frame.encrypted,
            crc_ok = frame.crc_ok,
            "received frame"
        );
        match frame.id {
            Some(id) if id <= MAX_REQUEST_ID => return Ok(self.complete(id, frame)),
            Some(id) if frame.crc_ok => {
                self.write(id, "ACK").await?;
                if let Some(event) = Event::parse(&frame.body) {
                    let _ = self.events.send(event);
                }
            }
            Some(id) => warn!(id, "discarding message with invalid checksum"),
            None => warn!(code = %frame.body, "panel reported an error"),
        }

        Ok(ControlFlow::Continue(()))
    }

    /// Pass a reply to the waiter for the request with the given ID
    fn complete(&mut self, id: u8, frame: Frame) -> ControlFlow<()> {
        let Some(request) = self.in_flight[usize::from(id - 1)].take() else {
            debug!(id, "ignoring reply to unknown request");
            return ControlFlow::Continue(());
        };

        let result = if !frame.crc_ok {
            Err(Error::Checksum)
        } else if frame.body.starts_with(['N', 'B']) {
            Err(Error::Panel(PanelError(frame.body)))
        } else {
            Ok(frame.body)
        };

        match request.waiter {
            Waiter::Command(reply) => {
                let _ = reply.send(result);
            }
            Waiter::KeepAlive => {
                if let Err(error) = result {
                    warn!(%error, "keepalive failed");
                }
            }
            Waiter::Disconnect => return ControlFlow::Break(()),
        }

        ControlFlow::Continue(())
    }

    /// Fail requests whose deadline has passed
    ///
    /// A keepalive timeout means the panel is unreachable, so it ends the connection.
    fn expire(&mut self) -> Result<ControlFlow<()>, Error> {
        let now = Instant::now();
        let mut flow = ControlFlow::Continue(());
        for slot in &mut self.in_flight {
            let Some(request) = slot.take_if(|request| request.deadline <= now) else {
                continue;
            };

            match request.waiter {
                Waiter::Command(reply) => {
                    let _ = reply.send(Err(Error::Timeout));
                }
                // When closing, the disconnect's own deadline ends the connection
                Waiter::KeepAlive if self.closing => {}
                Waiter::KeepAlive => {
                    warn!("keepalive timed out");
                    return Err(Error::Timeout);
                }
                Waiter::Disconnect => flow = ControlFlow::Break(()),
            }
        }

        Ok(flow)
    }

    async fn write(&mut self, id: u8, command: &str) -> Result<(), Error> {
        let frame = codec::encode(id, command, self.encrypted.then_some(&self.cipher))?;
        self.writer.write_all(&frame).await?;
        Ok(())
    }

    /// Read the next frame from the panel
    ///
    /// This is cancel-safe: buffered data is kept in `self.deframer`.
    async fn read_frame(&mut self) -> Result<Vec<u8>, Error> {
        loop {
            if let Some(frame) = self.deframer.next_frame() {
                return Ok(frame);
            }

            let mut buf = [0; 512];
            match self.reader.read(&mut buf).await? {
                0 => return Err(Error::Disconnected),
                n => {
                    trace!(data = %Hex(&buf[..n]), "received bytes");
                    self.deframer.extend(&buf[..n]);
                }
            }
        }
    }

    fn allocate_id(&mut self) -> u8 {
        loop {
            let id = self.next_id;
            self.next_id = if id == MAX_REQUEST_ID { 1 } else { id + 1 };
            if self.in_flight[usize::from(id - 1)].is_none() {
                return id;
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.in_flight.iter().flatten().map(|r| r.deadline).min()
    }

    fn in_flight_count(&self) -> usize {
        self.in_flight.iter().flatten().count()
    }

    fn keepalive_in_flight(&self) -> bool {
        self.in_flight
            .iter()
            .flatten()
            .any(|r| matches!(r.waiter, Waiter::KeepAlive))
    }
}

pub(crate) enum Request {
    Command {
        command: String,
        reply: oneshot::Sender<Result<String, Error>>,
    },
    Disconnect(oneshot::Sender<()>),
}

struct InFlight {
    waiter: Waiter,
    deadline: Instant,
}

enum Waiter {
    Command(oneshot::Sender<Result<String, Error>>),
    KeepAlive,
    Disconnect,
}

/// Formats bytes as space-separated hex
struct Hex<'a>(&'a [u8]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}
