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

/// Follows an external endpoint rate and changes the session format on the control plane.
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

    /// Stages a new session format and suspends processing for it. Nothing is saved until
    /// [`PreparedSessionFormat::commit`]; dropping the result abandons the change. The caller
    /// applies the native graph format between the two, and publishes DSP before resuming.
    /// # Errors
    /// Returns an unsupported format, a preference that cannot be staged, or a failed suspension.
    fn prepare_format(
        &self,
        _requested: RenderFormat,
    ) -> Result<Box<dyn PreparedSessionFormat + '_>, HostAudioBackendError> {
        Err(HostAudioBackendError::CapabilityUnavailable {
            capability: "live format reconfiguration".into(),
        })
    }
}

/// A session format change staged while processing is suspended.
pub trait PreparedSessionFormat: Send {
    /// Saves the change once the native graph accepted it. It cannot fail: the audio already
    /// runs at the new format, so an unsaved preference is reported as a session issue.
    fn commit(self: Box<Self>);
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
