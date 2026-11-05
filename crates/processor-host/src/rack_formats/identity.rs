//! Format and plug-in identities passed between the scanner, probe and DSP worker processes.

use std::path::PathBuf;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use mixrelay_control_protocol::ExternalProcessorFormat;
use serde::{Deserialize, Serialize};
use truce_rack_core::info::{PluginCategory, PluginInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RackFormat {
    Au,
    Auv3,
    Clap,
    Lv2,
    Vst3,
}

impl RackFormat {
    #[must_use]
    pub const fn argument(self) -> &'static str {
        match self {
            Self::Au => "au",
            Self::Auv3 => "auv3",
            Self::Clap => "clap",
            Self::Lv2 => "lv2",
            Self::Vst3 => "vst3",
        }
    }

    /// Parses the stable command-line representation used by scanner children.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested format is unknown to this host build.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "au" => Ok(Self::Au),
            "auv3" => Ok(Self::Auv3),
            "clap" => Ok(Self::Clap),
            "lv2" => Ok(Self::Lv2),
            "vst3" => Ok(Self::Vst3),
            _ => Err(format!("unsupported processor format: {value}")),
        }
    }

    pub(crate) const fn protocol(self) -> ExternalProcessorFormat {
        match self {
            Self::Au => ExternalProcessorFormat::Au,
            Self::Auv3 => ExternalProcessorFormat::Auv3,
            Self::Clap => ExternalProcessorFormat::Clap,
            Self::Lv2 => ExternalProcessorFormat::Lv2,
            Self::Vst3 => ExternalProcessorFormat::Vst3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RackCategory {
    Effect,
    Instrument,
    NoteEffect,
    Analyzer,
    Tool,
}

impl From<PluginCategory> for RackCategory {
    fn from(value: PluginCategory) -> Self {
        match value {
            PluginCategory::Effect => Self::Effect,
            PluginCategory::Instrument => Self::Instrument,
            PluginCategory::NoteEffect => Self::NoteEffect,
            PluginCategory::Analyzer => Self::Analyzer,
            PluginCategory::Tool => Self::Tool,
        }
    }
}

impl From<RackCategory> for PluginCategory {
    fn from(value: RackCategory) -> Self {
        match value {
            RackCategory::Effect => Self::Effect,
            RackCategory::Instrument => Self::Instrument,
            RackCategory::NoteEffect => Self::NoteEffect,
            RackCategory::Analyzer => Self::Analyzer,
            RackCategory::Tool => Self::Tool,
        }
    }
}

/// Serializable identity passed between the scanner, probe, and DSP worker processes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RackLocator {
    pub format: RackFormat,
    pub unique_id: String,
    pub path: PathBuf,
    pub(super) name: String,
    pub(super) vendor: String,
    pub(super) version: u32,
    pub(super) category: RackCategory,
    pub(super) has_editor: bool,
    pub(super) accepts_midi: bool,
}

impl RackLocator {
    pub(super) fn from_info(format: RackFormat, info: PluginInfo) -> Self {
        Self {
            format,
            unique_id: info.unique_id,
            path: info.path,
            name: info.name,
            vendor: info.vendor,
            version: info.version,
            category: info.category.into(),
            has_editor: info.has_editor,
            accepts_midi: info.accepts_midi,
        }
    }

    pub(super) fn info(&self) -> PluginInfo {
        PluginInfo {
            name: self.name.clone(),
            vendor: self.vendor.clone(),
            version: self.version,
            category: self.category.into(),
            path: self.path.clone(),
            unique_id: self.unique_id.clone(),
            format: self.format.argument(),
            has_editor: self.has_editor,
            accepts_midi: self.accepts_midi,
        }
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.name
    }

    pub(crate) fn is_audio_processor(&self) -> bool {
        self.category == RackCategory::Effect
            && match self.format {
                // Audio Unit generators and instruments can expose nominal stereo buses even
                // though they are not insert effects. Restrict the processor menu to the native
                // effect and music-effect component families.
                RackFormat::Au | RackFormat::Auv3 => {
                    self.unique_id.starts_with("aufx:") || self.unique_id.starts_with("aumf:")
                }
                RackFormat::Clap | RackFormat::Lv2 | RackFormat::Vst3 => true,
            }
    }

    /// Serializes this identity for a worker command-line argument.
    ///
    /// # Errors
    ///
    /// Returns an error if the identity cannot be serialized.
    pub fn encode(&self) -> Result<String, String> {
        serde_json::to_vec(self)
            .map(|bytes| BASE64.encode(bytes))
            .map_err(|error| error.to_string())
    }

    /// Restores a worker identity from its command-line representation.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed Base64 or invalid serialized identity data.
    pub fn decode(encoded: &str) -> Result<Self, String> {
        let bytes = BASE64.decode(encoded).map_err(|error| error.to_string())?;
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_FORMATS: [RackFormat; 5] = [
        RackFormat::Au,
        RackFormat::Auv3,
        RackFormat::Clap,
        RackFormat::Lv2,
        RackFormat::Vst3,
    ];

    fn locator(format: RackFormat, unique_id: &str, category: RackCategory) -> RackLocator {
        RackLocator {
            format,
            unique_id: unique_id.to_owned(),
            path: PathBuf::from("/fixture"),
            name: "Fixture".to_owned(),
            vendor: "MixRelay".to_owned(),
            version: 3,
            category,
            has_editor: true,
            accepts_midi: false,
        }
    }

    #[test]
    fn scanner_argument_names_every_format_and_rejects_unknown_ones() {
        for format in ALL_FORMATS {
            assert_eq!(RackFormat::parse(format.argument()), Ok(format));
        }
        assert_eq!(
            RackFormat::parse("aax"),
            Err("unsupported processor format: aax".to_owned())
        );
    }

    #[test]
    fn processor_menu_admits_effects_and_only_audio_unit_effect_families() {
        for format in [RackFormat::Clap, RackFormat::Lv2, RackFormat::Vst3] {
            assert!(locator(format, "any-id", RackCategory::Effect).is_audio_processor());
            assert!(!locator(format, "any-id", RackCategory::Instrument).is_audio_processor());
        }
        for format in [RackFormat::Au, RackFormat::Auv3] {
            assert!(locator(format, "aufx:lmtr:appl", RackCategory::Effect).is_audio_processor());
            assert!(locator(format, "aumf:dely:appl", RackCategory::Effect).is_audio_processor());
            // A generator that reports itself as an effect still stays out of insert slots.
            assert!(!locator(format, "augn:tone:appl", RackCategory::Effect).is_audio_processor());
            assert!(
                !locator(format, "aufx:lmtr:appl", RackCategory::Analyzer).is_audio_processor()
            );
        }
    }

    #[test]
    fn locator_round_trips_through_worker_argument() {
        let directory = mixrelay_test_support::FixtureDir::new("rack-locator").unwrap();
        let mut locator = locator(
            RackFormat::Clap,
            "io.mixrelay.fixture",
            RackCategory::Effect,
        );
        locator.path = directory.join("MixRelay Fixture.clap");

        let decoded = RackLocator::decode(&locator.encode().expect("locator should encode"))
            .expect("locator should decode");

        assert_eq!(decoded.format, locator.format);
        assert_eq!(decoded.unique_id, locator.unique_id);
        assert_eq!(decoded.path, locator.path);
        assert_eq!(decoded.name, locator.name);
        assert_eq!(decoded.vendor, locator.vendor);
    }
}
