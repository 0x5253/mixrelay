//! Processing-domain policy. Individual endpoint clocks/rates are observed and
//! bridged independently; no single HAL device defines this catalog.
use mixrelay_audio_io::{
    formats::{BUFFER_SIZES, SAMPLE_RATES},
    session::{AudioBackendKind, AudioSessionCapabilities, CURRENT_TIMER_FORMAT, ClockAuthority},
};
use mixrelay_engine::session::AdapterProfile;

pub(super) fn profile(rates: Vec<u32>) -> AdapterProfile {
    AdapterProfile {
        initial_format: CURRENT_TIMER_FORMAT,
        capabilities: AudioSessionCapabilities {
            backend: AudioBackendKind::CoreAudio,
            supported_sample_rates: rates,
            supported_block_frames: BUFFER_SIZES.to_vec(),
            can_reconfigure_live: true,
            hardware_input: true,
            hardware_output: true,
            virtual_endpoint: true,
            application_capture: true,
            sample_rate_conversion: true,
            drift_correction: true,
        },
        // Application capture owns a separate conversion boundary; virtual
        // device availability does not narrow its processing contract.
        application_capture_rates: SAMPLE_RATES.to_vec(),
        clock_authority: ClockAuthority::EngineTimer,
        // Scheduling/SRC latency belongs to each active clock boundary. There
        // is no fixed engine-wide queue delay to report for every route.
        transport_latency_frames: None,
        requires_observed_device_rate: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixrelay_audio_io::session::SessionIssue;
    use mixrelay_engine::{RuntimeStatusBackend, session::AdapterRuntimeStatus};

    #[test]
    fn processing_catalog_does_not_depend_on_one_virtual_devices_availability() {
        let profile = profile(SAMPLE_RATES.to_vec());
        assert_eq!(profile.capabilities.supported_sample_rates, SAMPLE_RATES);
        assert_eq!(profile.capabilities.supported_block_frames, BUFFER_SIZES);
        assert!(!profile.requires_observed_device_rate);
        assert_eq!(profile.application_capture_rates, SAMPLE_RATES);
        let runtime = AdapterRuntimeStatus::new(
            profile,
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        );
        let session = runtime
            .status(mixrelay_audio_graph::RenderFormat {
                sample_rate: CURRENT_TIMER_FORMAT.sample_rate,
                block_frames: CURRENT_TIMER_FORMAT.block_frames,
            })
            .audio_session
            .unwrap();
        assert_ne!(session.issue, Some(SessionIssue::DeviceUnavailable));
    }
}
