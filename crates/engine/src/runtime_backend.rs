//! Separate native observation, processing suspension and session-format transactions.
use mixrelay_audio_graph::RenderFormat;
use mixrelay_audio_io::{HostAudioBackendError, session::AudioSessionCapabilities};
use mixrelay_control_protocol::AppStatus;

/// Observes native health and capabilities without changing processing ownership.
pub trait RuntimeStatusBackend: Send + Sync {
    fn status(&self, format: RenderFormat) -> AppStatus;

    fn awaiting_microphone_permission(&self) -> bool {
        false
    }

    fn capabilities_for_format(&self, format: RenderFormat) -> Option<AudioSessionCapabilities> {
        self.status(format)
            .audio_session
            .map(|session| session.capabilities)
    }
}

/// Owns the native processing boundary during a control-plane transaction.
pub trait ProcessingControlBackend: Send + Sync {
    /// Quiesces processing at an audio boundary.
    /// # Errors
    /// Rejects unsupported suspension or an unacknowledged callback boundary.
    fn suspend(&self) -> Result<(), HostAudioBackendError> {
        Err(HostAudioBackendError::CapabilityUnavailable {
            capability: "audio processing suspension".into(),
        })
    }

    /// Keeps processing suspended after a rollback whose native state is unknown.
    fn fail_reconfiguration(&self) {}

    fn resume(&self) {}
}

/// Observes and negotiates native format on the control plane.
pub trait SessionFormatBackend: Send + Sync {
    /// Reads an explicitly selected external clock without writing its rate.
    fn external_sample_rate(&self) -> Option<u32> {
        None
    }

    fn external_format(&self, current: RenderFormat) -> Option<RenderFormat> {
        self.external_sample_rate().map(|sample_rate| RenderFormat {
            sample_rate,
            ..current
        })
    }

    /// Adopts an already changed native format without writing it back.
    /// # Errors
    /// Rejects unsupported followers or a clock that changes during preparation.
    fn adopt_external_format(&self, _requested: RenderFormat) -> Result<(), HostAudioBackendError> {
        Err(HostAudioBackendError::CapabilityUnavailable {
            capability: "external format following".into(),
        })
    }

    /// Negotiates native format while processing is suspended. Failure must restore the
    /// old format or retain suspension. The caller publishes DSP before resuming.
    /// # Errors
    /// Returns native negotiation or rollback failures.
    fn reconfigure(
        &self,
        _current: RenderFormat,
        _requested: RenderFormat,
    ) -> Result<(), HostAudioBackendError> {
        Err(HostAudioBackendError::CapabilityUnavailable {
            capability: "live format reconfiguration".into(),
        })
    }
}

/// One native owner implements all three contracts so a format transaction cannot combine
/// status from one device lifetime with processing or negotiation from another.
pub trait RuntimeBackend:
    RuntimeStatusBackend + ProcessingControlBackend + SessionFormatBackend
{
}

impl<T: RuntimeStatusBackend + ProcessingControlBackend + SessionFormatBackend> RuntimeBackend
    for T
{
}
