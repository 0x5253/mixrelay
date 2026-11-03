//! Driver inventory authorization and revision-checked native setting choices.
use mixrelay_audio_io::{
    ChannelLayout, NativeSampleFormat, VirtualEndpointDeviceSettings, VirtualEndpointId,
    VirtualEndpointSettings,
};
use mixrelay_driver_core::{DriverCommand, DriverSnapshot, DriverState};

/// One setting applied to every existing direction of the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingChoice {
    Channels(u16),
    SampleRate(u32),
    SampleFormat(NativeSampleFormat),
}

/// The observed identity and revision attached to one menu choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingAction {
    pub endpoint_id: VirtualEndpointId,
    pub expected_revision: u64,
    pub choice: SettingChoice,
}

/// Returns a channel count only when all existing directions agree.
#[must_use]
pub fn current_channels(device: &VirtualEndpointDeviceSettings) -> Option<u16> {
    match (device.spec.render, device.spec.capture) {
        (Some(render), Some(capture)) if render.channels == capture.channels => {
            Some(render.channels)
        }
        (Some(stream), None) | (None, Some(stream)) => Some(stream.channels),
        _ => None,
    }
}

/// Lists the intersection of channel capabilities for the existing directions.
#[must_use]
pub fn channel_choices(
    device: &VirtualEndpointDeviceSettings,
    settings: &VirtualEndpointSettings,
) -> Vec<u16> {
    let mut options = match (device.spec.render, device.spec.capture) {
        (Some(_), Some(_)) => settings
            .render_channels
            .iter()
            .copied()
            .filter(|count| settings.capture_channels.contains(count))
            .collect(),
        (Some(_), None) => settings.render_channels.clone(),
        (None, Some(_)) => settings.capture_channels.clone(),
        (None, None) => Vec::new(),
    };
    options.sort_unstable();
    options.dedup();
    options
}

/// Returns a PCM representation only when all existing directions have matching readback.
#[must_use]
pub fn current_sample_format(device: &VirtualEndpointDeviceSettings) -> Option<NativeSampleFormat> {
    let render = device
        .native_formats
        .render
        .current
        .map(|format| format.sample_format);
    let capture = device
        .native_formats
        .capture
        .current
        .map(|format| format.sample_format);
    match (device.spec.render, device.spec.capture) {
        (Some(_), Some(_)) if render == capture => render,
        (Some(_), None) => render,
        (None, Some(_)) => capture,
        _ => None,
    }
}

/// Lists PCM representations supported at the observed rate and each direction's channels.
#[must_use]
pub fn pcm_choices(device: &VirtualEndpointDeviceSettings) -> Vec<NativeSampleFormat> {
    let Some(rate) = device.observed_sample_rate else {
        return Vec::new();
    };
    if device.spec.render.is_none() && device.spec.capture.is_none() {
        return Vec::new();
    }
    [
        NativeSampleFormat::Signed16,
        NativeSampleFormat::Signed24,
        NativeSampleFormat::Signed32,
        NativeSampleFormat::Float32,
    ]
    .into_iter()
    .filter(|sample_format| {
        [
            (device.spec.render, &device.native_formats.render),
            (device.spec.capture, &device.native_formats.capture),
        ]
        .into_iter()
        .all(|(stream, formats)| {
            let Some(stream) = stream else {
                return true;
            };
            if formats.current.is_none() {
                return false;
            }
            formats.supported.iter().any(|format| {
                format.sample_rate == rate
                    && format.channels == stream.channels
                    && format.sample_format == *sample_format
            })
        })
    })
    .collect()
}

/// Names bit depth while distinguishing integer and float representations.
#[must_use]
pub const fn pcm_label(format: NativeSampleFormat) -> &'static str {
    match format {
        NativeSampleFormat::Signed16 => "16 bit",
        NativeSampleFormat::Signed24 => "24 bit",
        NativeSampleFormat::Signed32 => "32 bit (integer)",
        NativeSampleFormat::Float32 => "32 bit (float)",
    }
}

/// Formats Hz as kHz, without redundant decimals for integral kilohertz.
#[must_use]
pub fn rate_label(rate: u32) -> String {
    if rate.is_multiple_of(1_000) {
        format!("{} kHz", rate / 1_000)
    } else {
        format!("{} kHz", f64::from(rate) / 1_000.0)
    }
}

/// Derives one supported device change from fresh readback at the displayed revision.
/// Returns an error for changed revisions, unavailable devices, or unsupported values.
/// # Errors
/// Rejects stale revisions, unavailable devices, and unsupported setting values.
pub fn command(
    snapshot: &DriverSnapshot,
    action: &SettingAction,
) -> Result<DriverCommand, DeviceChoiceError> {
    if snapshot.state != DriverState::Ready {
        return Err(DeviceChoiceError::DriverUnavailable);
    }
    if snapshot.revision != action.expected_revision {
        return Err(DeviceChoiceError::RevisionChanged);
    }
    let device = snapshot
        .settings
        .devices
        .iter()
        .find(|device| device.spec.id == action.endpoint_id)
        .ok_or(DeviceChoiceError::MissingDevice)?;
    if let SettingChoice::SampleFormat(sample_format) = action.choice {
        if !pcm_choices(device).contains(&sample_format) {
            return Err(DeviceChoiceError::UnsupportedPcm);
        }
        return Ok(DriverCommand::SetSampleFormat {
            expected_revision: snapshot.revision,
            endpoint_id: action.endpoint_id.clone(),
            sample_format,
        });
    }
    configure(snapshot, device, action.choice)
}

fn configure(
    snapshot: &DriverSnapshot,
    device: &VirtualEndpointDeviceSettings,
    choice: SettingChoice,
) -> Result<DriverCommand, DeviceChoiceError> {
    let mut spec = device.spec.clone();
    match choice {
        SettingChoice::Channels(channels) => {
            spec.default_sample_rate = device
                .observed_sample_rate
                .ok_or(DeviceChoiceError::MissingRate)?;
            if !channel_choices(device, &snapshot.settings).contains(&channels) {
                return Err(DeviceChoiceError::UnsupportedChannels);
            }
            for stream in [&mut spec.render, &mut spec.capture].into_iter().flatten() {
                stream.channels = channels;
                stream.channel_layout = match channels {
                    1 => ChannelLayout::Mono,
                    2 => ChannelLayout::Stereo,
                    _ => ChannelLayout::Discrete,
                };
            }
        }
        SettingChoice::SampleRate(rate) => {
            if !device.can_set_sample_rate || !device.supported_sample_rates.contains(&rate) {
                return Err(DeviceChoiceError::UnsupportedRate);
            }
            spec.default_sample_rate = rate;
        }
        SettingChoice::SampleFormat(_) => {
            return Err(DeviceChoiceError::PcmRequiresNative);
        }
    }
    Ok(DriverCommand::Configure {
        expected_revision: snapshot.revision,
        spec,
    })
}

/// A displayed device choice can be applied only to its current supported readback.
#[derive(Debug, thiserror::Error)]
pub enum DeviceChoiceError {
    #[error("Driver is unavailable")]
    DriverUnavailable,
    #[error("Device settings changed; choose again")]
    RevisionChanged,
    #[error("The device is no longer available")]
    MissingDevice,
    #[error("The native PCM format is no longer supported")]
    UnsupportedPcm,
    #[error("The current sample rate is unavailable")]
    MissingRate,
    #[error("The channel count is no longer supported")]
    UnsupportedChannels,
    #[error("The sample rate is no longer supported")]
    UnsupportedRate,
    #[error("PCM changes require a native format command")]
    PcmRequiresNative,
}

/// An absent installed owner preserves install/version facts while exposing no capabilities.
#[must_use]
pub fn absent_owner_snapshot(installed: bool, installed_version: Option<String>) -> DriverSnapshot {
    DriverSnapshot {
        protocol_version: 1,
        revision: 0,
        state: if installed {
            DriverState::InstalledNotLoaded
        } else {
            DriverState::NotInstalled
        },
        installed_version,
        owner_process_id: None,
        settings: VirtualEndpointSettings {
            can_create: false,
            can_rename: false,
            render_channels: Vec::new(),
            capture_channels: Vec::new(),
            creation_template: None,
            devices: Vec::new(),
        },
        persistence_error: None,
    }
}

#[cfg(test)]
mod tests;
