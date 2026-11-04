//! Graph endpoint transfers inside one native callback. Each endpoint is served
//! by a bridged device, the callback's own buffers, or the shared source registry.
use super::{Binding, buffers::Buffers};
use crate::macos_devices::DeviceSet;
use mixrelay_audio_io::process::{InputActivity, ProcessCycle, TransferOutcome};
use mixrelay_render::{CycleAdmission, ExternalBinding, ExternalControlState, ExternalIo};

pub(super) struct CallbackIo<'a> {
    pub(super) buffers: Buffers<'a>,
    pub(super) bindings: &'a [Binding],
    /// Frame offset of the current graph quantum within the native callback.
    pub(super) offset: usize,
    /// Bridged devices and the host time labelling their transfers in this callback.
    pub(super) bridged: Option<(&'a DeviceSet, u64)>,
}

impl<'a> CallbackIo<'a> {
    /// The callback buffer mapping for a graph endpoint, if this device owns it.
    fn native_binding(&self, binding: &ExternalBinding, input: bool) -> Option<&'a Binding> {
        self.bindings
            .iter()
            .find(|map| map.input == input && map.id == binding.node_id().as_str())
    }
}

impl ExternalIo for CallbackIo<'_> {
    fn source_epoch(&self, binding: &ExternalBinding) -> Option<u64> {
        // The native binding owns its clock and continuity. A registry epoch
        // belongs only to a source actually read from the shared transport.
        let native = self.native_binding(binding, true).is_some()
            || self.bridged.is_some_and(|(devices, _)| {
                devices.binding_present(binding.node_id().as_str(), true)
            });
        if native {
            return None;
        }
        crate::macos::application_epoch(binding, crate::endpoint_service::source_registry())
    }

    fn begin_cycle(
        &mut self,
        _cycle: &ProcessCycle,
        controls: ExternalControlState<'_>,
    ) -> CycleAdmission {
        if let Some((devices, _)) = self.bridged {
            for (binding, enabled) in controls.sources() {
                devices.set_binding_active(binding.node_id().as_str(), true, enabled);
            }
            for (binding, enabled) in controls.sinks() {
                devices.set_binding_active(binding.node_id().as_str(), false, enabled);
            }
        }
        CycleAdmission::Process
    }

    fn read_source(
        &mut self,
        binding: &ExternalBinding,
        destination: &mut [f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        if let Some(result) = self
            .bridged
            .and_then(|(devices, host)| devices.read(binding.node_id().as_str(), destination, host))
        {
            return result;
        }
        let Some(map) = self.native_binding(binding, true) else {
            return crate::macos::read_application(
                binding,
                destination,
                cycle,
                crate::endpoint_service::source_registry(),
                self.bridged.map(|(_, host)| host),
            );
        };
        if map.offset.is_none() {
            // An offline device leaves the engine's zeroed buffer silent.
            return TransferOutcome {
                activity: InputActivity::Inactive,
                ..TransferOutcome::complete(cycle.frames)
            };
        }
        self.buffers.transfer(map, self.offset, destination);
        TransferOutcome::complete(cycle.frames)
    }

    fn write_sink(
        &mut self,
        binding: &ExternalBinding,
        source: &[f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        if let Some(result) = self
            .bridged
            .and_then(|(devices, host)| devices.write(binding.node_id().as_str(), source, host))
        {
            return result;
        }
        let Some(map) = self.native_binding(binding, false) else {
            return TransferOutcome::complete(0);
        };
        // Multiple graph sinks may map to the same physical channels: mix their
        // contributions into the callback's initially zeroed output, not overwrite.
        self.buffers.write(map, self.offset, source);
        TransferOutcome::complete(cycle.frames)
    }
}
