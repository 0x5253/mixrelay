//! Projects adapter observations and active process counters into control status.

use super::{
    AdapterRuntimeStatus, AppStatus, AudioSessionCapabilities, AudioSessionStatus,
    ClockObservation, DriverState, Ordering, PROTOCOL_VERSION, RenderFormat, RuntimeStatusBackend,
    SessionFormat, SessionIssue, WorkerState, unpack_format,
};

impl AdapterRuntimeStatus {
    pub(super) fn session_issue(
        &self,
        format: RenderFormat,
        observed: RenderFormat,
        worker_state: WorkerState,
        clock_issue: Option<SessionIssue>,
    ) -> Option<SessionIssue> {
        if worker_state == WorkerState::Failed {
            Some(SessionIssue::ProcessingFailed)
        } else if self.failed.load(Ordering::Acquire) {
            Some(SessionIssue::ReconfigurationFailed)
        } else if self.application_state.load(Ordering::Acquire) == 2 {
            Some(SessionIssue::ApplicationAmbiguous)
        } else if self.application_state.load(Ordering::Acquire) == 1 {
            Some(SessionIssue::ApplicationUnavailable)
        } else if self.restore_failed.load(Ordering::Acquire) && self.routing_clock_rate().is_none()
        {
            Some(SessionIssue::StoredFormatUnavailable)
        } else if self.preferences_error.load(Ordering::Acquire) {
            Some(SessionIssue::PreferencesUnavailable)
        } else if self.profile.requires_observed_device_rate {
            if observed.sample_rate == 0 {
                Some(SessionIssue::DeviceUnavailable)
            } else if observed.sample_rate != format.sample_rate {
                Some(SessionIssue::SampleRateMismatch)
            } else if observed.block_frames != 0 && observed.block_frames != format.block_frames {
                Some(SessionIssue::BlockSizeMismatch)
            } else {
                clock_issue
            }
        } else {
            clock_issue
        }
    }
}
impl AdapterRuntimeStatus {
    pub(super) fn capabilities(&self, format: RenderFormat) -> AudioSessionCapabilities {
        let mut capabilities = self.profile.capabilities.clone();
        capabilities.supported_sample_rates = self.supported_sample_rates();
        if let Some(rate) = self.routing_clock_rate() {
            // This API reports the selected hardware clock. Changing that
            // device's native rate is a device operation, not an own-HAL write.
            capabilities.supported_sample_rates = self
                .profile
                .capabilities
                .supported_sample_rates
                .iter()
                .copied()
                .filter(|supported| *supported == rate)
                .collect();
        }
        capabilities.application_capture &= self
            .profile
            .application_capture_rates
            .contains(&format.sample_rate);
        capabilities.can_reconfigure_live &= self.native.can_reconfigure();
        capabilities
    }
}

impl RuntimeStatusBackend for AdapterRuntimeStatus {
    fn capabilities_for_format(&self, format: RenderFormat) -> Option<AudioSessionCapabilities> {
        Some(self.capabilities(format))
    }
    fn status(&self, format: RenderFormat) -> AppStatus {
        let format = if self.native.owns_format() {
            unpack_format(self.active_format.load(Ordering::Acquire))
        } else {
            format
        };
        let observed = self.routing_clock_rate().map_or_else(
            || unpack_format(self.observed_device_format.load(Ordering::Acquire)),
            |sample_rate| RenderFormat {
                sample_rate,
                block_frames: 0,
            },
        );
        let worker_state = self.processing.state();
        let clock = ClockObservation::unpack(self.clock_observation.load(Ordering::Acquire));
        let clock_issue =
            (!clock.idle && !clock.synchronized).then_some(SessionIssue::ClockNotSynchronized);
        let observed_device_sample_rate =
            (observed.sample_rate != 0).then_some(observed.sample_rate);
        let observed_device_buffer_frames =
            (observed.block_frames != 0).then_some(observed.block_frames);
        let issue = self.session_issue(format, observed, worker_state, clock_issue);
        let process = self.telemetry.snapshot();
        let capabilities = self.capabilities(format);
        AppStatus {
            engine_power: None,
            processor_timings: Vec::new(),
            processor_latencies: Vec::new(),
            failed_processors: Vec::new(),
            recorder_error: None,
            protocol_version: PROTOCOL_VERSION,
            driver_state: match (
                capabilities.virtual_endpoint,
                self.endpoint_observation.load(Ordering::Acquire),
            ) {
                (true, 1) => DriverState::Ready,
                (true, 3) => DriverState::Unavailable,
                (true, 0) => DriverState::InstalledNotLoaded,
                _ => DriverState::NotInstalled,
            },
            // An operational DSP engine is independent from virtual-driver installation.
            sample_rate: Some(format.sample_rate),
            buffer_frames: Some(format.block_frames),
            underruns: process.input_starvations,
            overruns: process.output_overflows,
            sample_faults: process.sample_faults,
            audio_session: Some(AudioSessionStatus {
                endpoint_preferences_unavailable: false,
                virtual_endpoint_settings: None,
                processing_state: match worker_state {
                    WorkerState::Running if !clock.idle => {
                        mixrelay_audio_io::process::ProcessingState::Running
                    }
                    WorkerState::Stopped | WorkerState::Failed => {
                        mixrelay_audio_io::process::ProcessingState::Stopped
                    }
                    WorkerState::NotStarted | WorkerState::Running => {
                        mixrelay_audio_io::process::ProcessingState::Idle
                    }
                },
                discontinuities: process.discontinuities,
                format: SessionFormat {
                    sample_rate: format.sample_rate,
                    block_frames: format.block_frames,
                },
                preferred_format: self.preferred_format(),
                clock_authority: clock.authority,
                observed_device_sample_rate,
                observed_device_buffer_frames,
                issue,
                transport_latency_frames: self.profile.transport_latency_frames,
                capabilities,
            }),
        }
    }
}
