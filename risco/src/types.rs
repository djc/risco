use std::fmt;
use std::str::FromStr;

use crate::Error;

/// An unsolicited message from the panel
#[derive(Debug)]
#[non_exhaustive]
pub enum Event {
    /// A zone's status changed
    Zone { id: u16, status: ZoneStatus },
    /// A partition's status changed
    Partition { id: u8, status: PartitionStatus },
    /// The system status changed
    System(SystemStatus),
    /// An entry was added to the panel's event log (raw text, as sent by the panel)
    Log(String),
    /// Any other message, as sent by the panel
    Other(String),
    /// The connection failed
    ///
    /// This is always the last event. When the connection is closed deliberately (through
    /// [`Client::disconnect()`](crate::Client::disconnect) or by dropping all clients),
    /// the event stream ends without it.
    Disconnected(Error),
}

impl Event {
    /// Parse an unsolicited message; returns `None` for messages that should be ignored
    pub(crate) fn parse(message: &str) -> Option<Self> {
        if message.starts_with("CLOCK") {
            return None;
        }

        if let Some(entry) = message.strip_prefix("EVENT=") {
            return Some(Self::Log(entry.to_owned()));
        }

        let Some((key, value)) = message.split_once('=') else {
            return Some(Self::Other(message.to_owned()));
        };

        let event = if let Some(id) = key.strip_prefix("ZSTT") {
            parse_id(id).map(|id| Self::Zone {
                id,
                status: ZoneStatus::new(value),
            })
        } else if let Some(id) = key.strip_prefix("PSTT") {
            parse_id(id).map(|id| Self::Partition {
                id,
                status: PartitionStatus::new(value),
            })
        } else if key.starts_with("SSTT") {
            Some(Self::System(SystemStatus::new(value)))
        } else {
            None
        };

        Some(event.unwrap_or_else(|| Self::Other(message.to_owned())))
    }
}

/// A zone (detector or other input) configured on the panel
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Zone {
    /// Zone ID, starting at 1
    pub id: u16,
    /// The zone's name
    pub label: String,
    /// The zone type as configured on the panel (for example, 1 for exit/entry or 5 for instant)
    pub zone_type: u8,
    /// How the zone is connected to the panel, as reported by the panel (not reported by
    /// Agility, WiComm and LightSys Air panels)
    pub technology: Option<String>,
    /// The partitions the zone belongs to
    pub partitions: Vec<u8>,
    /// The groups the zone belongs to
    pub groups: Vec<Group>,
    /// The zone's status at the time it was queried
    pub status: ZoneStatus,
}

/// A partition (an independently armed area) configured on the panel
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Partition {
    /// Partition ID, starting at 1
    pub id: u8,
    /// The partition's name
    pub label: String,
    /// The partition's status at the time it was queried
    pub status: PartitionStatus,
}

/// The panel itself
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct System {
    /// The system's name
    pub label: String,
    /// The system status at the time it was queried
    pub status: SystemStatus,
}

/// A group of zones within a partition, which can be armed separately
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    A,
    B,
    C,
    D,
}

impl Group {
    const ALL: [Self; 4] = [Self::A, Self::B, Self::C, Self::D];

    /// The 1-based number the panel uses for this group
    pub(crate) fn number(self) -> u8 {
        self as u8 + 1
    }

    /// Parse a hex bitmask of groups
    pub(crate) fn parse_mask(value: &str) -> Vec<Self> {
        let mask = u8::from_str_radix(value.trim(), 16).unwrap_or_default();
        Self::ALL
            .into_iter()
            .filter(|group| mask & (1 << (group.number() - 1)) != 0)
            .collect()
    }
}

impl fmt::Display for Group {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// Parse a partition bitmask: each hex digit covers four partitions, lowest bit first
pub(crate) fn parse_partitions(value: &str) -> Vec<u8> {
    value
        .trim()
        .chars()
        .enumerate()
        .flat_map(|(i, digit)| {
            let bits = digit.to_digit(16).unwrap_or_default();
            (0..4)
                .filter(move |bit| bits & (1 << bit) != 0)
                .filter_map(move |bit| u8::try_from(i * 4 + bit + 1).ok())
        })
        .collect()
}

macro_rules! status {
    (
        $(#[$meta:meta])*
        $name:ident {
            $( $(#[$doc:meta])* $method:ident => $flag:literal, )*
        }
    ) => {
        $(#[$meta])*
        ///
        /// The panel reports status as a string of flag characters. The raw string is available
        /// through `as_str()` for flags this type doesn't cover.
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            pub(crate) fn new(status: &str) -> Self {
                Self(status.trim().to_owned())
            }

            /// The raw status string
            pub fn as_str(&self) -> &str {
                &self.0
            }

            $(
                $(#[$doc])*
                pub fn $method(&self) -> bool {
                    self.0.contains($flag)
                }
            )*
        }
    };
}

status! {
    /// The status of a zone
    ZoneStatus {
        /// The zone is open, or its detector is triggered (`O`)
        is_open => 'O',
        /// The zone is armed (`A`)
        is_armed => 'A',
        /// The zone is causing an alarm (`a`)
        is_alarmed => 'a',
        /// The zone is bypassed (`Y`)
        is_bypassed => 'Y',
        /// The zone's device is tampered with (`T`)
        is_tampered => 'T',
        /// The zone has a trouble condition (`R`)
        has_trouble => 'R',
        /// The zone's wireless device is lost (`L`)
        is_lost => 'L',
        /// The zone's wireless device has a low battery (`b`)
        has_low_battery => 'b',
        /// The zone has a communication trouble (`C`)
        has_comm_trouble => 'C',
        /// The zone is in soak test (`S`)
        in_soak_test => 'S',
        /// The zone is a 24-hour zone (`H`)
        is_24_hour => 'H',
    }
}

status! {
    /// The status of a partition
    PartitionStatus {
        /// The partition exists (`E`)
        exists => 'E',
        /// The partition is fully armed (`A`)
        is_armed => 'A',
        /// The partition is partially armed, in stay or home mode (`H`)
        is_partially_armed => 'H',
        /// The partition is ready to be armed (`R`)
        is_ready => 'R',
        /// The partition is in alarm (`a`)
        is_alarmed => 'a',
        /// One of the partition's zones is open (`O`)
        has_open_zone => 'O',
        /// A duress alarm was raised (`D`)
        is_duress => 'D',
        /// A false code was entered (`C`)
        has_false_code => 'C',
        /// A fire alarm was raised (`F`)
        is_fire => 'F',
        /// A panic alarm was raised (`P`)
        is_panic => 'P',
        /// A medical alarm was raised (`M`)
        is_medical => 'M',
        /// An alarm in memory needs to be reset (`S`)
        needs_reset => 'S',
        /// No activity was detected for the configured period (`N`)
        has_no_activity => 'N',
        /// The partition has a trouble condition (`T`)
        has_trouble => 'T',
    }
}

impl PartitionStatus {
    /// The partition is neither fully nor partially armed
    pub fn is_disarmed(&self) -> bool {
        !self.is_armed() && !self.is_partially_armed()
    }

    /// The given group is armed
    pub fn is_group_armed(&self, group: Group) -> bool {
        self.0.contains(char::from(b'0' + group.number()))
    }
}

status! {
    /// The status of the panel itself
    SystemStatus {
        /// The panel's battery is low (`B`)
        has_low_battery => 'B',
        /// The panel has lost mains power (`A`)
        has_ac_trouble => 'A',
        /// The phone line has a trouble condition (`P`)
        has_phone_line_trouble => 'P',
        /// The panel's clock is not set (`C`)
        has_clock_trouble => 'C',
        /// Reporting to monitoring station 1 failed (`1`)
        has_monitoring_station_1_trouble => '1',
        /// Reporting to monitoring station 2 failed (`2`)
        has_monitoring_station_2_trouble => '2',
        /// Reporting to monitoring station 3 failed (`3`)
        has_monitoring_station_3_trouble => '3',
        /// The panel's enclosure is open (`X`)
        is_box_tampered => 'X',
        /// Wireless jamming was detected (`J`)
        has_jamming => 'J',
        /// The panel is in programming mode (`I`)
        in_programming_mode => 'I',
        /// The panel is in learn mode (`L`)
        in_learn_mode => 'L',
        /// The panel is in walk test mode (`W`)
        in_walk_test => 'W',
        /// The auxiliary power output has a trouble condition (`U`)
        has_aux_trouble => 'U',
        /// The RS-485 bus has a trouble condition (`R`)
        has_bus_trouble => 'R',
        /// The bell has a trouble condition (`E`)
        has_bell_trouble => 'E',
        /// The bell is tampered with (`Y`)
        is_bell_tampered => 'Y',
    }
}

/// Parse the ID from a status key like `ZSTT12` or `PSTT*3` (with the prefix removed)
fn parse_id<T: FromStr>(id: &str) -> Option<T> {
    id.trim_start_matches('*').parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_events() {
        assert!(Event::parse("CLOCK=05/11/2026 12:00").is_none());
        assert!(
            matches!(Event::parse("EVENT=Zone 3 open"), Some(Event::Log(e)) if e == "Zone 3 open")
        );
        let Some(Event::Zone { id: 12, status }) = Event::parse("ZSTT12=--O-A------") else {
            panic!("expected zone event");
        };
        assert!(status.is_open() && status.is_armed() && !status.is_bypassed());
        let Some(Event::Partition { id: 2, status }) = Event::parse("PSTT*2=E--H-3") else {
            panic!("expected partition event");
        };
        assert!(status.is_partially_armed() && !status.is_disarmed());
        assert!(status.is_group_armed(Group::C) && !status.is_group_armed(Group::A));
        assert!(matches!(Event::parse("SSTT=--A--"), Some(Event::System(s)) if s.has_ac_trouble()));
        assert!(matches!(Event::parse("OSTT1=a"), Some(Event::Other(_))));
        assert!(matches!(Event::parse("ZSTTx=O"), Some(Event::Other(_))));
        assert!(matches!(Event::parse("garbage"), Some(Event::Other(_))));
    }

    #[test]
    fn bitmasks() {
        assert_eq!(parse_partitions("1"), [1]);
        assert_eq!(parse_partitions("3"), [1, 2]);
        assert_eq!(parse_partitions("08"), [8]);
        assert_eq!(parse_partitions("F1"), [1, 2, 3, 4, 5]);
        assert_eq!(Group::parse_mask("5"), [Group::A, Group::C]);
        assert_eq!(Group::parse_mask("0"), []);
    }
}
