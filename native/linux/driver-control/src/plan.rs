//! Native-independent validation and normalized direction formats for Driver owners.

use mixrelay_audio_io::{
    HostAudioContractError, NativeSampleFormat, NativeStreamFormat, VirtualEndpointCapabilities,
};
use mixrelay_driver_control::DriverDeviceConfig;

#[derive(Debug, thiserror::Error)]
pub(super) enum PlanError {
    #[error(transparent)]
    Contract(#[from] HostAudioContractError),
    #[error("native tuple differs from device direction/nominal rate")]
    DirectionTuple,
}

pub(super) fn capabilities() -> VirtualEndpointCapabilities {
    VirtualEndpointCapabilities {
        max_render_channels: 64,
        max_capture_channels: 64,
        supported_sample_rates: mixrelay_audio_io::formats::SAMPLE_RATES.to_vec(),
        supports_duplex: true,
        supports_dynamic_creation: true,
    }
}

#[derive(Debug, Clone)]
pub(super) struct DevicePlan {
    pub(super) config: DriverDeviceConfig,
    pub(super) formats: [Option<NativeStreamFormat>; 2],
}

impl DevicePlan {
    pub(super) fn new(config: &DriverDeviceConfig) -> Result<Self, PlanError> {
        config.spec.validate(&capabilities())?;
        for (shape, requested) in [
            (config.spec.render, config.render_format),
            (config.spec.capture, config.capture_format),
        ] {
            if requested.is_some_and(|requested| {
                shape.is_none_or(|shape| shape.channels != requested.channels)
                    || requested.sample_rate != config.spec.default_sample_rate
            }) {
                return Err(PlanError::DirectionTuple);
            }
        }
        Ok(Self {
            config: config.clone(),
            formats: requested_formats(config),
        })
    }

    pub(super) fn matches(&self, current: &DriverDeviceConfig) -> bool {
        self.config.spec == current.spec && self.formats == requested_formats(current)
    }
}

pub(super) fn requested_formats(config: &DriverDeviceConfig) -> [Option<NativeStreamFormat>; 2] {
    let default = |shape: mixrelay_audio_io::StreamSpec| NativeStreamFormat {
        channels: shape.channels,
        sample_rate: config.spec.default_sample_rate,
        sample_format: NativeSampleFormat::Float32,
    };
    [
        config
            .render_format
            .or_else(|| config.spec.render.map(default)),
        config
            .capture_format
            .or_else(|| config.spec.capture.map(default)),
    ]
}

#[cfg(test)]
pub(crate) mod tests;
