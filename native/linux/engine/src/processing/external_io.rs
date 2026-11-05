//! Routes the DSP plan's external ports to the native owner of each graph node.
use crate::{applications::CaptureSet, devices::DeviceStreams, endpoints::EndpointSet};
use mixrelay_audio_graph::{AUDIO_PORT_ID, FROM_OS_PORT_ID, TO_OS_PORT_ID};
use mixrelay_audio_io::process::{ProcessCycle, TransferOutcome};
use mixrelay_render::{ExternalBinding, ExternalIo};

pub(super) struct LinuxExternalIo<'a> {
    pub(super) endpoints: &'a mut EndpointSet,
    pub(super) application_captures: &'a mut CaptureSet,
    pub(super) devices: &'a mut DeviceStreams,
}

impl ExternalIo for LinuxExternalIo<'_> {
    /// Virtual devices read through their OS port; an audio port belongs to an
    /// application capture when one owns the node, otherwise to a physical input.
    fn read_source(
        &mut self,
        binding: &ExternalBinding,
        destination: &mut [f32],
        _cycle: &ProcessCycle,
    ) -> TransferOutcome {
        let node = binding.node_id().as_str();
        match binding.port_id().as_str() {
            FROM_OS_PORT_ID => self.endpoints.read(node, destination),
            AUDIO_PORT_ID if self.application_captures.contains(node) => {
                self.application_captures.read(node, destination)
            }
            AUDIO_PORT_ID => self.devices.read(node, destination),
            _ => TransferOutcome::complete(0),
        }
    }

    fn write_sink(
        &mut self,
        binding: &ExternalBinding,
        source: &[f32],
        _cycle: &ProcessCycle,
    ) -> TransferOutcome {
        let node = binding.node_id().as_str();
        match binding.port_id().as_str() {
            TO_OS_PORT_ID => self.endpoints.write(node, source),
            AUDIO_PORT_ID => self.devices.write(node, source),
            _ => TransferOutcome::complete(0),
        }
    }
}
