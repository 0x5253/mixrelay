//! Shared provider-bus identity and channel-geometry contract.

use serde::{Deserialize, Serialize};

use crate::model::{AUDIO_PORT_ID, PortId};

/// Maximum UTF-8 byte length of a stable provider layout identity.
pub const MAX_PROVIDER_LAYOUT_ID_BYTES: usize = 256;
/// Maximum UTF-8 byte length of a stable provider bus identity.
pub const MAX_PROVIDER_BUS_ID_BYTES: usize = 128;
/// Maximum UTF-8 byte length of a provider bus display name.
pub const MAX_PROVIDER_BUS_NAME_BYTES: usize = 1_024;
/// Maximum number of buses allowed in either direction.
pub const MAX_PROVIDER_BUSES_PER_DIRECTION: usize = 64;
/// Maximum channels in a bus and the aggregate per direction.
pub const MAX_PROVIDER_CHANNELS_PER_DIRECTION: u16 = 64;

/// Authored plug-in I/O geometry; contains no native handles or machine-specific addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderPorts {
    /// Stable identity of the complete selected bus arrangement.
    pub layout_id: String,
    /// Input buses in the provider's declared order.
    pub inputs: Vec<ProviderBus>,
    /// Output buses in the provider's declared order.
    pub outputs: Vec<ProviderBus>,
}

/// One ordered provider audio bus. Direction is established by its containing list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderBus {
    /// Stable direction-local port identity.
    pub id: PortId,
    /// Human-readable label; empty names may use a presentation fallback.
    pub name: String,
    /// Semantic role of this bus.
    pub kind: ProviderBusKind,
    /// Number of interleaved channels carried by this bus.
    pub channels: u16,
    /// Semantic speaker arrangement when it is a standard layout.
    pub channel_format: ProviderChannelFormat,
}

/// Semantic role assigned to a provider bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderBusKind {
    /// Primary program input or output.
    Main,
    /// Auxiliary signal used to control or influence processing.
    Sidechain,
    /// Additional input or output bus.
    Auxiliary,
}

/// Standard speaker arrangement or discrete channel geometry for one bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderChannelFormat {
    /// One-channel mono arrangement.
    Mono,
    /// Two-channel stereo arrangement.
    Stereo,
    /// Six-channel 5.1 arrangement.
    Surround5_1,
    /// Eight-channel 7.1 arrangement.
    Surround7_1,
    /// Discrete channels without a standard speaker interpretation.
    Discrete,
}

/// Typed validation failures for a provider bus layout.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderPortsError {
    /// A required layout or bus identity was empty.
    #[error("provider {identity} must not be empty")]
    EmptyIdentity {
        /// Identity category that was empty.
        identity: &'static str,
    },
    /// An identity or display name exceeded its byte bound.
    #[error("provider {identity} exceeds {maximum} bytes (found {found})")]
    IdentityTooLong {
        /// Identity category that exceeded its bound.
        identity: &'static str,
        /// Maximum permitted UTF-8 bytes.
        maximum: usize,
        /// Actual UTF-8 byte length.
        found: usize,
    },
    /// A layout or bus identity contains a control character, or a name contains NUL.
    #[error("provider {identity} contains a NUL or control character")]
    InvalidIdentity {
        /// Identity category that contained an invalid character.
        identity: &'static str,
    },
    /// A direction contains no buses.
    #[error("provider {direction} must contain at least one bus")]
    NoBuses {
        /// Direction that was empty.
        direction: &'static str,
    },
    /// A direction contains more buses than the configured maximum.
    #[error("provider {direction} has {found} buses; maximum is {maximum}")]
    TooManyBuses {
        /// Direction that exceeded the bus count.
        direction: &'static str,
        /// Maximum permitted number of buses.
        maximum: usize,
        /// Actual number of buses.
        found: usize,
    },
    /// A bus channel count is outside the supported range.
    #[error("provider {direction} bus {bus_id} has invalid channel count {channels}")]
    InvalidBusChannels {
        /// Direction containing the invalid bus.
        direction: &'static str,
        /// Identity of the invalid bus.
        bus_id: PortId,
        /// Unsupported channel count.
        channels: u16,
    },
    /// A direction contains the same port identity more than once.
    #[error("provider {direction} bus {bus_id} has duplicate identity")]
    DuplicateBusId {
        /// Direction containing duplicate identities.
        direction: &'static str,
        /// Duplicated identity.
        bus_id: PortId,
    },
    /// The total channel count for one direction exceeds its bound.
    #[error("provider {direction} has {channels} total channels; maximum is {maximum}")]
    TooManyTotalChannels {
        /// Direction that exceeded its channel total.
        direction: &'static str,
        /// Maximum permitted aggregate channel count.
        maximum: u16,
        /// Actual aggregate channel count.
        channels: u16,
    },
    /// A standard channel format does not match its required number of channels.
    #[error(
        "provider {direction} bus {bus_id} channel format {channel_format:?} does not match {channels} channels"
    )]
    ChannelFormatMismatch {
        /// Direction containing the mismatched bus.
        direction: &'static str,
        /// Identity of the mismatched bus.
        bus_id: PortId,
        /// Declared channel format.
        channel_format: ProviderChannelFormat,
        /// Actual channel count.
        channels: u16,
    },
}

impl ProviderPorts {
    /// Describes a one-bus stereo layout with the canonical audio port identity.
    #[must_use]
    pub fn canonical_stereo(layout_id: impl Into<String>) -> Self {
        Self::single_bus(layout_id, 2, 2)
    }

    /// Describes a conventional one-bus layout, using discrete format for other channel counts.
    #[must_use]
    pub fn single_bus(layout_id: impl Into<String>, inputs: u16, outputs: u16) -> Self {
        let make_bus = |channels| ProviderBus {
            id: PortId::from(AUDIO_PORT_ID),
            name: "Main".to_owned(),
            kind: ProviderBusKind::Main,
            channels,
            channel_format: match channels {
                1 => ProviderChannelFormat::Mono,
                2 => ProviderChannelFormat::Stereo,
                6 => ProviderChannelFormat::Surround5_1,
                8 => ProviderChannelFormat::Surround7_1,
                _ => ProviderChannelFormat::Discrete,
            },
        };
        Self {
            layout_id: layout_id.into(),
            inputs: vec![make_bus(inputs)],
            outputs: vec![make_bus(outputs)],
        }
    }

    /// Validates identities, channel formats, and per-direction resource bounds.
    ///
    /// # Errors
    /// Returns a typed error for malformed identities or unsupported geometry.
    pub fn validate(&self) -> Result<(), ProviderPortsError> {
        validate_identity("layout id", &self.layout_id, MAX_PROVIDER_LAYOUT_ID_BYTES)?;
        validate_buses("input", &self.inputs)?;
        validate_buses("output", &self.outputs)
    }

    /// Returns the validated aggregate number of input channels.
    ///
    /// # Errors
    /// Returns a typed error when the complete layout is invalid.
    pub fn total_input_channels(&self) -> Result<u16, ProviderPortsError> {
        self.total_channels(&self.inputs)
    }

    /// Returns the validated aggregate number of output channels.
    ///
    /// # Errors
    /// Returns a typed error when the complete layout is invalid.
    pub fn total_output_channels(&self) -> Result<u16, ProviderPortsError> {
        self.total_channels(&self.outputs)
    }

    fn total_channels(&self, buses: &[ProviderBus]) -> Result<u16, ProviderPortsError> {
        self.validate()?;
        Ok(buses.iter().map(|bus| bus.channels).sum())
    }
}

/// Bounds required by saved layouts and compiled provider ports.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderGeometryError {
    #[error("provider direction must contain at least one bus")]
    NoBuses,
    #[error("provider direction has {found} buses; maximum is {MAX_PROVIDER_BUSES_PER_DIRECTION}")]
    TooManyBuses { found: usize },
    #[error("provider bus {index} has invalid channel count {channels}")]
    InvalidBusChannels { index: usize, channels: u16 },
    #[error(
        "provider direction has {channels} total channels; maximum is {MAX_PROVIDER_CHANNELS_PER_DIRECTION}"
    )]
    TooManyTotalChannels { channels: u16 },
}

/// Validates bus count, individual widths and aggregate width without requiring bus identities.
///
/// # Errors
/// Returns the first unsupported count or width in the supplied bus order.
pub fn validate_provider_geometry(
    channels: impl ExactSizeIterator<Item = u16>,
) -> Result<u16, ProviderGeometryError> {
    let count = channels.len();
    if count == 0 {
        return Err(ProviderGeometryError::NoBuses);
    }
    if count > MAX_PROVIDER_BUSES_PER_DIRECTION {
        return Err(ProviderGeometryError::TooManyBuses { found: count });
    }
    let mut total = 0;
    for (index, channels) in channels.enumerate() {
        if !(1..=MAX_PROVIDER_CHANNELS_PER_DIRECTION).contains(&channels) {
            return Err(ProviderGeometryError::InvalidBusChannels { index, channels });
        }
        // At most 64 buses of 64 channels reach this addition.
        total += channels;
        if total > MAX_PROVIDER_CHANNELS_PER_DIRECTION {
            return Err(ProviderGeometryError::TooManyTotalChannels { channels: total });
        }
    }
    Ok(total)
}

fn validate_buses(
    direction: &'static str,
    buses: &[ProviderBus],
) -> Result<(), ProviderPortsError> {
    validate_provider_geometry(buses.iter().map(|bus| bus.channels)).map_err(
        |error| match error {
            ProviderGeometryError::NoBuses => ProviderPortsError::NoBuses { direction },
            ProviderGeometryError::TooManyBuses { found } => ProviderPortsError::TooManyBuses {
                direction,
                maximum: MAX_PROVIDER_BUSES_PER_DIRECTION,
                found,
            },
            ProviderGeometryError::InvalidBusChannels { index, channels } => {
                ProviderPortsError::InvalidBusChannels {
                    direction,
                    bus_id: buses[index].id.clone(),
                    channels,
                }
            }
            ProviderGeometryError::TooManyTotalChannels { channels } => {
                ProviderPortsError::TooManyTotalChannels {
                    direction,
                    maximum: MAX_PROVIDER_CHANNELS_PER_DIRECTION,
                    channels,
                }
            }
        },
    )?;
    for (index, bus) in buses.iter().enumerate() {
        validate_identity("bus id", bus.id.as_str(), MAX_PROVIDER_BUS_ID_BYTES)?;
        validate_display_name(&bus.name)?;
        if buses[..index].iter().any(|previous| previous.id == bus.id) {
            return Err(ProviderPortsError::DuplicateBusId {
                direction,
                bus_id: bus.id.clone(),
            });
        }
        if !channel_count_matches(bus.channel_format, bus.channels) {
            return Err(ProviderPortsError::ChannelFormatMismatch {
                direction,
                bus_id: bus.id.clone(),
                channel_format: bus.channel_format,
                channels: bus.channels,
            });
        }
    }
    Ok(())
}

fn validate_identity(
    identity: &'static str,
    value: &str,
    maximum: usize,
) -> Result<(), ProviderPortsError> {
    if value.is_empty() {
        return Err(ProviderPortsError::EmptyIdentity { identity });
    }
    if value.len() > maximum {
        return Err(ProviderPortsError::IdentityTooLong {
            identity,
            maximum,
            found: value.len(),
        });
    }
    if value.chars().any(char::is_control) {
        return Err(ProviderPortsError::InvalidIdentity { identity });
    }
    Ok(())
}

fn validate_display_name(value: &str) -> Result<(), ProviderPortsError> {
    if value.len() > MAX_PROVIDER_BUS_NAME_BYTES {
        return Err(ProviderPortsError::IdentityTooLong {
            identity: "bus name",
            maximum: MAX_PROVIDER_BUS_NAME_BYTES,
            found: value.len(),
        });
    }
    if value.contains('\0') {
        return Err(ProviderPortsError::InvalidIdentity {
            identity: "bus name",
        });
    }
    Ok(())
}

const fn channel_count_matches(format: ProviderChannelFormat, channels: u16) -> bool {
    match format {
        ProviderChannelFormat::Mono => channels == 1,
        ProviderChannelFormat::Stereo => channels == 2,
        ProviderChannelFormat::Surround5_1 => channels == 6,
        ProviderChannelFormat::Surround7_1 => channels == 8,
        ProviderChannelFormat::Discrete => true,
    }
}
