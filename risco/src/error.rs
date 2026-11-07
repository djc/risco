use std::{fmt, io};

/// Errors returned when communicating with a panel
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The TCP connection failed
    Io(io::Error),
    /// The panel replied with an error code
    Panel(PanelError),
    /// The panel rejected the access code
    Unauthorized,
    /// The panel did not reply in time
    Timeout,
    /// The panel still had an encrypted session from an earlier, unclean disconnect
    ///
    /// The session was ended to reset the panel; a fresh connection attempt should now succeed.
    StaleSession,
    /// The connection to the panel has been closed
    Disconnected,
    /// The reply's checksum did not match its contents
    Checksum,
    /// The command cannot be sent to the panel
    InvalidCommand(&'static str),
    /// The panel sent a reply that could not be interpreted
    UnexpectedResponse(String),
    /// The panel type reported by the panel is not known
    UnsupportedPanel(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Panel(error) => error.fmt(f),
            Self::Unauthorized => f.write_str("access code rejected by panel"),
            Self::Timeout => f.write_str("timed out waiting for panel"),
            Self::StaleSession => {
                f.write_str("panel had a stale encrypted session; it was reset, retry shortly")
            }
            Self::Disconnected => f.write_str("disconnected from panel"),
            Self::Checksum => f.write_str("invalid checksum in reply from panel"),
            Self::InvalidCommand(reason) => write!(f, "invalid command: {reason}"),
            Self::UnexpectedResponse(response) => {
                write!(f, "unexpected response from panel: {response:?}")
            }
            Self::UnsupportedPanel(panel_type) => {
                write!(f, "unsupported panel type {panel_type:?}")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Panel(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// An error code sent by the panel in reply to a command
///
/// Codes start with `N` (for example, `N12` when arming a partition that is not ready) or `B`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelError(pub(crate) String);

impl PanelError {
    /// The raw error code, such as `N05`
    pub fn code(&self) -> &str {
        &self.0
    }

    /// A description of the error code, if it is known
    pub fn description(&self) -> Option<&'static str> {
        Some(match self.0.as_str() {
            "BCK2" => "callback error",
            "N01" => "error",
            "N04" => "CRC error",
            "N05" => "invalid parameter",
            "N06" => "invalid value",
            "N07" => "system armed",
            "N08" => "system alarm",
            "N09" => "default jumper",
            "N10" => "system not in programming mode",
            "N11" => "system in programming mode",
            "N12" => "system not ready to arm",
            "N13" => "general error",
            "N14" => "device does not support this operation",
            "N15" => "monitoring station locked",
            "N16" => "system busy",
            "N17" => "PIN code in use",
            "N18" => "system in RF allocation mode",
            "N19" => "device does not exist",
            "N20" => "TEOL termination not supported",
            "N24" => "system in remote upgrade",
            "N25" => "CW test failed",
            _ => return None,
        })
    }
}

impl fmt::Display for PanelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.description() {
            Some(description) => write!(f, "panel error {} ({description})", self.0),
            None => write!(f, "panel error {}", self.0),
        }
    }
}

impl std::error::Error for PanelError {}
