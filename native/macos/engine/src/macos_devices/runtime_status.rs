//! Runtime status and processing control, observed through the current routing clock.
use super::MacDevices;
use mixrelay_audio_graph::RenderFormat;
use mixrelay_audio_io::HostAudioBackendError;
use mixrelay_engine_core::{ProcessingControlBackend, RuntimeStatusBackend};

impl RuntimeStatusBackend for MacDevices {
    fn capabilities_for_format(
        &self,
        format: RenderFormat,
    ) -> Option<mixrelay_audio_io::session::AudioSessionCapabilities> {
        self.status
            .observe_routing_clock(self.follows_hardware(), self.routing_rate());
        self.status.capabilities_for_format(format)
    }
    fn awaiting_microphone_permission(&self) -> bool {
        self.host.permission_waiting()
    }
    fn status(&self, format: RenderFormat) -> mixrelay_control_protocol::AppStatus {
        self.status
            .observe_routing_clock(self.follows_hardware(), self.routing_rate());
        self.status.status(format)
    }
}

impl ProcessingControlBackend for MacDevices {
    fn fail_reconfiguration(&self) {
        self.status.fail_reconfiguration();
    }
    fn suspend(&self) -> Result<(), HostAudioBackendError> {
        self.status.pause_at_boundary()
    }
    fn resume(&self) {
        self.status.resume();
    }
}
