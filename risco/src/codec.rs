//! Framing, encryption and checksums for the panel's TCP protocol
//!
//! A frame is `STX [0x11] payload ETX`, where the optional `0x11` byte marks an encrypted
//! payload. The payload is a two-digit command ID, the command text, an ETB separator and a
//! CRC-16 over everything up to and including the separator, as four uppercase hex digits.
//! Encryption XORs the payload with a keystream derived from the panel ID. After encryption,
//! STX, ETX and DLE bytes in the payload are escaped with a DLE prefix.

use std::collections::VecDeque;
use std::{iter, mem};

use crate::Error;

const STX: u8 = 0x02;
const ETX: u8 = 0x03;
const DLE: u8 = 0x10;
const ENCRYPTED: u8 = 0x11;
const ETB: u8 = 0x17;

/// The highest ID the client uses for requests; the panel uses higher IDs for its own messages
pub(crate) const MAX_REQUEST_ID: u8 = 49;

/// Frames longer than this are assumed to be garbage
const MAX_FRAME_LEN: usize = 1024;

/// The XOR keystream used for encrypted frames
#[derive(Clone)]
pub(crate) struct Cipher([u8; Self::LEN]);

impl Cipher {
    const LEN: usize = 255;

    /// Derive the keystream from the panel ID (as returned by the `RID` command)
    ///
    /// Panel ID 0 yields an all-zero keystream, which leaves payloads unchanged.
    pub(crate) fn new(panel_id: u16) -> Self {
        let mut key = [0; Self::LEN];
        for (byte, value) in key.iter_mut().zip(keystream(panel_id)) {
            *byte = value;
        }
        Self(key)
    }

    fn apply(&self, data: &mut [u8]) {
        debug_assert!(data.len() <= Self::LEN);
        for (byte, key) in data.iter_mut().zip(self.0) {
            *byte ^= key;
        }
    }
}

/// Encode a command into a frame, encrypting it if a `cipher` is given
pub(crate) fn encode(id: u8, command: &str, cipher: Option<&Cipher>) -> Result<Vec<u8>, Error> {
    if command.bytes().any(|b| b.is_ascii_control()) {
        return Err(Error::InvalidCommand("contains control characters"));
    }

    let mut payload = format!("{id:02}{command}\x17").into_bytes();
    let crc = crc16(&payload);
    payload.extend_from_slice(format!("{crc:04X}").as_bytes());

    let mut frame = Vec::with_capacity(payload.len() + 8);
    frame.push(STX);
    if let Some(cipher) = cipher {
        if payload.len() > Cipher::LEN {
            return Err(Error::InvalidCommand("too long to encrypt"));
        }
        cipher.apply(&mut payload);
        frame.push(ENCRYPTED);
    }

    for byte in payload {
        if matches!(byte, STX | ETX | DLE) {
            frame.push(DLE);
        }
        frame.push(byte);
    }

    frame.push(ETX);
    Ok(frame)
}

/// A 16-bit LFSR with taps on bits 1, 2, 4 and 15, seeded with the panel ID
fn keystream(panel_id: u16) -> impl Iterator<Item = u8> {
    let mut state = panel_id;
    iter::repeat_with(move || {
        let feedback = (state & 0x8016).count_ones() as u16 & 1;
        state = state << 1 | feedback;
        state as u8
    })
}

/// Decode an unescaped frame payload (as yielded by [`Deframer`])
pub(crate) fn decode(payload: &[u8], cipher: &Cipher) -> Result<Frame, &'static str> {
    let encrypted = payload.first() == Some(&ENCRYPTED);
    let mut plain = payload[usize::from(encrypted)..].to_vec();
    if encrypted {
        if plain.len() > Cipher::LEN {
            return Err("too long to decrypt");
        }
        cipher.apply(&mut plain);
    }

    let Some((message, crc_ok)) = verify(&plain) else {
        return Err("missing checksum separator");
    };

    // Error codes for commands the panel could not attribute have no ID
    let (id, body) = match message {
        [b'N' | b'B', ..] => (None, message),
        [tens @ b'0'..=b'9', ones @ b'0'..=b'9', body @ ..] => {
            match (tens - b'0') * 10 + (ones - b'0') {
                0 => return Err("invalid command ID"),
                id => (Some(id), body),
            }
        }
        _ => return Err("invalid command ID"),
    };

    Ok(Frame {
        encrypted,
        id,
        body: String::from_utf8_lossy(body).into_owned(),
        crc_ok,
    })
}

/// Find the panel ID that decrypts an encrypted payload by trying all of them
///
/// The panel can keep encrypting after a connection is closed, in which case it also encrypts
/// its reply to the `RID` command that we need to learn the panel ID. Only an ID that turns the
/// payload into a message with a command ID and a valid checksum is accepted.
pub(crate) fn find_panel_id(payload: &[u8]) -> Option<u16> {
    let [ENCRYPTED, ciphertext @ ..] = payload else {
        return None;
    };
    if ciphertext.len() > Cipher::LEN {
        return None;
    }

    let mut plain = vec![0; ciphertext.len()];
    (1..=u16::MAX).find(|&panel_id| {
        for ((plain, cipher), key) in plain.iter_mut().zip(ciphertext).zip(keystream(panel_id)) {
            *plain = cipher ^ key;
        }

        matches!(verify(&plain), Some(([b'0'..=b'9', b'0'..=b'9', ..], true)))
    })
}

/// Split a decrypted payload into the message and whether its checksum is valid
fn verify(plain: &[u8]) -> Option<(&[u8], bool)> {
    let separator = plain.iter().position(|&b| b == ETB)?;
    let (message, checksum) = plain.split_at(separator + 1);
    let crc_ok = format!("{:04X}", crc16(message)).as_bytes() == checksum;
    Some((&message[..separator], crc_ok))
}

pub(crate) struct Frame {
    pub(crate) encrypted: bool,
    pub(crate) id: Option<u8>,
    pub(crate) body: String,
    pub(crate) crc_ok: bool,
}

/// Splits a byte stream into frames, removing STX/ETX delimiters and DLE escapes
#[derive(Default)]
pub(crate) struct Deframer {
    state: State,
    current: Vec<u8>,
    complete: VecDeque<Vec<u8>>,
}

impl Deframer {
    pub(crate) fn extend(&mut self, data: &[u8]) {
        for &byte in data {
            self.state = match (self.state, byte) {
                (State::Idle, STX) | (State::Frame, STX) => {
                    if self.state == State::Frame {
                        tracing::warn!("discarding incomplete frame");
                    }
                    self.current.clear();
                    State::Frame
                }
                (State::Idle, _) => State::Idle,
                (State::Frame, DLE) => State::Escape,
                (State::Frame, ETX) => {
                    self.complete.push_back(mem::take(&mut self.current));
                    State::Idle
                }
                (State::Frame | State::Escape, _) if self.current.len() >= MAX_FRAME_LEN => {
                    tracing::warn!("discarding oversized frame");
                    self.current.clear();
                    State::Idle
                }
                (State::Frame | State::Escape, _) => {
                    self.current.push(byte);
                    State::Frame
                }
            };
        }
    }

    pub(crate) fn next_frame(&mut self) -> Option<Vec<u8>> {
        self.complete.pop_front()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// Waiting for STX
    #[default]
    Idle,
    /// Inside a frame
    Frame,
    /// Inside a frame, after a DLE
    Escape,
}

/// CRC-16/MODBUS (reflected polynomial 0xA001, initial value 0xFFFF)
fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFF;
    for &byte in data {
        crc ^= u16::from(byte);
        for _ in 0..8 {
            crc = match crc & 1 {
                0 => crc >> 1,
                _ => crc >> 1 ^ 0xA001,
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_check_value() {
        assert_eq!(crc16(b"123456789"), 0x4B37);
    }

    #[test]
    fn keystream() {
        // Worked by hand from the reference implementations' pseudo-buffer algorithm
        assert_eq!(Cipher::new(1).0[..8], [2, 5, 11, 23, 47, 94, 189, 122]);
        assert!(Cipher::new(0).0.iter().all(|&b| b == 0));
    }

    #[test]
    fn plain_frame_layout() {
        let frame = encode(1, "RID", None).unwrap();
        let crc = format!("{:04X}", crc16(b"01RID\x17"));
        let mut expected = vec![STX];
        expected.extend_from_slice(b"01RID\x17");
        expected.extend_from_slice(crc.as_bytes());
        expected.push(ETX);
        assert_eq!(frame, expected);
    }

    #[test]
    fn round_trip() {
        let mut escaped = 0;
        for panel_id in 0..500 {
            let cipher = Cipher::new(panel_id);
            for (id, encrypt) in [(7, false), (49, true)] {
                let frame = encode(id, "ZSTT*12?", encrypt.then_some(&cipher)).unwrap();
                escaped += frame[1..frame.len() - 1]
                    .iter()
                    .filter(|&&b| b == DLE)
                    .count();

                let mut deframer = Deframer::default();
                deframer.extend(&frame);
                let frame = decode(&deframer.next_frame().unwrap(), &cipher).unwrap();
                assert!(frame.crc_ok);
                assert_eq!(frame.encrypted, encrypt);
                assert_eq!(frame.id, Some(id));
                assert_eq!(frame.body, "ZSTT*12?");
                assert!(deframer.next_frame().is_none());
            }
        }

        // Make sure the escaping paths were exercised
        assert!(escaped > 0);
    }

    #[test]
    fn deframe_escapes_and_noise() {
        let mut deframer = Deframer::default();
        // Noise, then a frame containing an escaped DLE followed by an escaped ETX
        deframer.extend(&[0xff, STX, b'a', DLE, DLE, DLE]);
        assert!(deframer.next_frame().is_none());
        deframer.extend(&[ETX, ETX, STX, b'b']);
        assert_eq!(deframer.next_frame().unwrap(), [b'a', DLE, ETX]);
        assert!(deframer.next_frame().is_none());
        // An unescaped STX restarts the frame
        deframer.extend(&[STX, b'c', ETX]);
        assert_eq!(deframer.next_frame().unwrap(), [b'c']);
    }

    #[test]
    fn decode_errors() {
        let cipher = Cipher::new(0);
        let frame = decode(b"N05\x170000", &cipher).unwrap();
        assert_eq!(frame.id, None);
        assert_eq!(frame.body, "N05");
        assert!(!frame.crc_ok);

        assert!(decode(b"01ACK", &cipher).is_err());
        assert!(decode(b"00ACK\x170000", &cipher).is_err());
    }

    #[test]
    fn recover_panel_id() {
        for panel_id in [1, 0x1234, u16::MAX] {
            let frame = encode(1, "RID=0001", Some(&Cipher::new(panel_id))).unwrap();
            let mut deframer = Deframer::default();
            deframer.extend(&frame);
            assert_eq!(
                find_panel_id(&deframer.next_frame().unwrap()),
                Some(panel_id)
            );
        }

        let frame = encode(1, "RID=0001", None).unwrap();
        assert_eq!(find_panel_id(&frame[1..frame.len() - 1]), None);
    }

    #[test]
    fn reject_control_characters() {
        assert!(encode(1, "RMT=12\x0334", None).is_err());
    }
}
