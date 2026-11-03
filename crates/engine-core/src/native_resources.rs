//! Native graph preparation contracts and portable fallback backends.

use crate::{
    AppStatus, Arc, DriverState, GraphDocument, HostAudioBackendError, NodeConfig,
    PROTOCOL_VERSION, ProcessingControlBackend, RenderFormat, RuntimeStatusBackend,
    SessionFormatBackend, VirtualEndpointBackend, VirtualEndpointCapabilities, VirtualEndpointSpec,
    graph_composition,
};

pub struct ContractOnlyVirtualEndpointBackend {
    capabilities: VirtualEndpointCapabilities,
}

/// One prepared plan's graph-local playback intents and exact runtime controls.
/// Preserve has no playback lease or bank. Forward always refers to the prepared
/// processor's actual control bank; adapters must reject unsupported forwarding.
pub enum PlaybackBindings<'a> {
    /// No graph requests playback suppression.
    Preserve,
    /// Suppression intents and the control bank of the same prepared execution.
    Forward {
        /// Graph-local intents independent of canonical capture identity.
        intents: &'a [graph_composition::ApplicationPlaybackIntent],
        /// Live controls of this exact plan; missing or disabled masks grant no lease.
        controls: &'a Arc<mixrelay_render::RuntimeControlBank>,
    },
}

impl<'a> PlaybackBindings<'a> {
    pub(super) fn from_plan(
        intents: &'a [graph_composition::ApplicationPlaybackIntent],
        controls: &'a Arc<mixrelay_render::RuntimeControlBank>,
    ) -> Self {
        if intents.is_empty() {
            Self::Preserve
        } else {
            Self::Forward { intents, controls }
        }
    }
}

/// Prepares operating-system resources that are coupled to one complete graph document.
///
/// Endpoint-only validation remains in [`VirtualEndpointBackend`]. This second boundary lets a
/// platform atomically stage endpoint transports and application-capture sessions together,
/// without teaching the portable graph or DSP crates about `PipeWire` or Core Audio.
pub trait GraphResourceBackend: Send + Sync {
    /// True only after native graph owners, including quarantined teardown, are released.
    fn execution_released(&self) -> bool {
        true
    }
    /// Resolves requested native identities for canonical hardware ownership.
    /// # Errors
    /// Rejects adapters without native device enumeration.
    fn device_inventory(
        &self,
        _selectors: &[&mixrelay_audio_io::DeviceSelector],
    ) -> Result<Vec<mixrelay_audio_io::AudioDeviceDescriptor>, HostAudioBackendError> {
        Err(HostAudioBackendError::CapabilityUnavailable {
            capability: "native device identity resolution".into(),
        })
    }

    /// Resolves an application capture policy to one canonical zero-gain native source.
    /// # Errors
    /// Rejects selectors or capture policies that the adapter cannot fulfill exactly.
    fn resolve_application_capture(
        &self,
        _request: &mixrelay_audio_io::ApplicationCaptureRequest,
    ) -> Result<graph_composition::SharedSource, HostAudioBackendError> {
        Err(HostAudioBackendError::CapabilityUnavailable {
            capability: "application capture identity resolution".into(),
        })
    }
    /// Stages every native resource required by `document` on the control thread.
    ///
    /// # Errors
    ///
    /// Returns a host error before the prepared processor is published to the audio thread.
    fn reconcile(
        &self,
        document: &GraphDocument,
        playback: PlaybackBindings<'_>,
    ) -> Result<(), HostAudioBackendError>;

    /// Prepares format-dependent native transports off RT before native negotiation.
    /// Dropping the result must cancel only this staging, leaving active owners intact.
    /// Implementations must reserve their publication slot here so `commit` cannot fail.
    /// # Errors
    /// Rejects unsupported bindings or native preparation/capacity failures.
    fn prepare_format_change(
        &self,
        document: &GraphDocument,
        playback: PlaybackBindings<'_>,
        current: RenderFormat,
        requested: RenderFormat,
    ) -> Result<Box<dyn PreparedGraphFormat>, HostAudioBackendError> {
        if matches!(playback, PlaybackBindings::Forward { .. }) {
            return Err(HostAudioBackendError::CapabilityUnavailable {
                capability: "format-changing application playback control".into(),
            });
        }
        if current != requested
            && document.nodes.iter().any(|node| {
                matches!(
                    node.config,
                    NodeConfig::ApplicationCapture(_)
                        | NodeConfig::AudioDeviceInput(_)
                        | NodeConfig::AudioDeviceOutput(_)
                )
            })
        {
            return Err(HostAudioBackendError::CapabilityUnavailable {
                capability: "format-changing capture/device transport".into(),
            });
        }
        Ok(Box::new(UnchangedGraphFormat))
    }
}

/// Reserved native format handoff. Created and dropped only on the control plane.
/// `commit` is called while DSP is suspended, immediately before its new plan publishes.
/// It must be infallible: Rust DSP/FIFO allocation, validation and mailbox reservation happen
/// in `prepare_format_change`. Fallible native mapping/thread startup and bounded ownership
/// bookkeeping run after suspension in `configure_native`, which verifies restoration of prior
/// native ownership on failure. The native consumer must activate at the matching graph revision.
pub trait PreparedGraphFormat: Send {
    /// Apply checked native client settings after DSP suspension. An error must
    /// roll back changes owned by this token before returning to the controller.
    /// # Errors
    /// Rejects native settings that failed actual readback or rollback.
    fn configure_native(&mut self) -> Result<(), HostAudioBackendError> {
        Ok(())
    }
    fn commit(self: Box<Self>);
}

struct UnchangedGraphFormat;
impl PreparedGraphFormat for UnchangedGraphFormat {
    fn commit(self: Box<Self>) {}
}

#[derive(Debug)]
pub struct FixedRuntimeStatus {
    driver_state: DriverState,
}

impl FixedRuntimeStatus {
    #[must_use]
    pub const fn new(driver_state: DriverState) -> Self {
        Self { driver_state }
    }
}

impl RuntimeStatusBackend for FixedRuntimeStatus {
    fn status(&self, format: RenderFormat) -> AppStatus {
        let ready = self.driver_state == DriverState::Ready;
        AppStatus {
            engine_power: None,
            processor_timings: Vec::new(),
            processor_latencies: Vec::new(),
            failed_processors: Vec::new(),
            recorder_error: None,
            protocol_version: PROTOCOL_VERSION,
            driver_state: self.driver_state,
            sample_rate: ready.then_some(format.sample_rate),
            buffer_frames: ready.then_some(format.block_frames),
            underruns: 0,
            overruns: 0,
            sample_faults: mixrelay_audio_io::process::SampleFaults::default(),
            audio_session: None,
        }
    }
}

impl ProcessingControlBackend for FixedRuntimeStatus {}

impl SessionFormatBackend for FixedRuntimeStatus {}

#[derive(Debug, Default)]
pub struct NoopGraphResourceBackend;

impl GraphResourceBackend for NoopGraphResourceBackend {
    fn reconcile(
        &self,
        _document: &GraphDocument,
        playback: PlaybackBindings<'_>,
    ) -> Result<(), HostAudioBackendError> {
        if matches!(playback, PlaybackBindings::Forward { .. }) {
            return Err(HostAudioBackendError::CapabilityUnavailable {
                capability: "application playback control".into(),
            });
        }
        Ok(())
    }
}

impl Default for ContractOnlyVirtualEndpointBackend {
    fn default() -> Self {
        Self {
            capabilities: VirtualEndpointCapabilities {
                max_render_channels: 64,
                max_capture_channels: 64,
                supported_sample_rates: vec![44_100, 48_000],
                supports_duplex: true,
                supports_dynamic_creation: false,
            },
        }
    }
}

impl VirtualEndpointBackend for ContractOnlyVirtualEndpointBackend {
    fn capabilities(&self) -> &VirtualEndpointCapabilities {
        &self.capabilities
    }

    fn prepare_graph_attachments(
        &self,
        _desired: &[VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError> {
        Ok(())
    }
}
