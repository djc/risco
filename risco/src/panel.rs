use std::fmt;

use crate::Error;

/// Information about the connected panel
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Panel {
    /// The panel model
    pub model: Model,
    /// The panel type as reported by the panel (for example, `RP432`)
    pub panel_type: String,
    /// The firmware version (not reported by Agility, WiComm and LightSys Air panels)
    pub firmware: Option<String>,
    /// The panel's serial number
    pub serial: String,
    /// The highest zone ID supported by this panel
    pub max_zones: u16,
    /// The highest partition ID supported by this panel
    pub max_partitions: u8,
    /// The highest output ID supported by this panel
    pub max_outputs: u16,
}

impl Panel {
    pub(crate) fn new(
        panel_type: String,
        firmware: Option<String>,
        serial: String,
    ) -> Result<Self, Error> {
        let version = firmware.as_deref().map(parse_version).unwrap_or_default();
        let (model, max_zones, max_partitions, max_outputs) =
            match panel_type.split(':').next().unwrap_or_default() {
                "RW032" => (Model::Agility4, 32, 3, 4),
                "RW132" => (Model::Agility, 36, 3, 4),
                "RW232" => (Model::WiComm, 36, 3, 4),
                "RW332" => (Model::WiCommPro, 36, 3, 4),
                "RP432" => match version.first() {
                    Some(3..) => (Model::LightSys, 50, 4, 32),
                    _ => (Model::LightSys, 32, 4, 14),
                },
                "RP432MP" => (Model::LightSysPlus, 512, 32, 196),
                "RW432MPV" => (Model::LightSysAir, 128, 32, 4),
                "RP512" if version.as_slice() >= [1, 2, 0, 7].as_slice() => {
                    (Model::ProSysPlus, 128, 32, 262)
                }
                "RP512" => (Model::ProSysPlus, 64, 32, 262),
                _ => return Err(Error::UnsupportedPanel(panel_type)),
            };

        Ok(Self {
            model,
            panel_type,
            firmware,
            serial,
            max_zones,
            max_partitions,
            max_outputs,
        })
    }

    /// Older panels don't support some of the commands used for querying zones
    pub(crate) fn is_legacy(&self) -> bool {
        is_legacy(&self.panel_type)
    }
}

pub(crate) fn is_legacy(panel_type: &str) -> bool {
    !panel_type.starts_with("RP")
}

/// Parse a firmware version like `1.2.0.7` into its numeric components
fn parse_version(firmware: &str) -> Vec<u32> {
    firmware
        .split('.')
        .map_while(|part| part.parse().ok())
        .collect()
}

/// A panel model
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Model {
    /// Agility 4 (`RW032`)
    Agility4,
    /// Agility (`RW132`)
    Agility,
    /// WiComm (`RW232`)
    WiComm,
    /// WiComm Pro (`RW332`)
    WiCommPro,
    /// LightSYS (`RP432`)
    LightSys,
    /// LightSYS Plus (`RP432MP`)
    LightSysPlus,
    /// LightSYS Air (`RW432MPV`)
    LightSysAir,
    /// ProSYS Plus or GT Plus (`RP512`)
    ProSysPlus,
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Agility4 => "Agility 4",
            Self::Agility => "Agility",
            Self::WiComm => "WiComm",
            Self::WiCommPro => "WiComm Pro",
            Self::LightSys => "LightSYS",
            Self::LightSysPlus => "LightSYS Plus",
            Self::LightSysAir => "LightSYS Air",
            Self::ProSysPlus => "ProSYS Plus",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(panel_type: &str, firmware: Option<&str>) -> Panel {
        Panel::new(
            panel_type.to_owned(),
            firmware.map(str::to_owned),
            "0".to_owned(),
        )
        .unwrap()
    }

    #[test]
    fn capabilities() {
        assert_eq!(panel("RW132:1", None).model, Model::Agility);
        assert!(panel("RW432MPV", None).is_legacy());
        assert_eq!(panel("RP432", Some("2.10")).max_zones, 32);
        assert_eq!(panel("RP432", Some("3.47")).max_zones, 50);
        assert_eq!(panel("RP512", Some("1.2.0.6")).max_zones, 64);
        assert_eq!(panel("RP512", Some("1.2.0.7")).max_zones, 128);
        assert_eq!(panel("RP512", Some("1.3")).max_zones, 128);
        assert!(!panel("RP512", None).is_legacy());
        assert!(matches!(
            Panel::new("XX1".into(), None, "0".into()),
            Err(Error::UnsupportedPanel(_))
        ));
    }
}
