//! Bounded native LV2 DSP port binding and processing.

use crate::{
    Lv2Plugin,
    ports::{AudioPlanes, PortKind},
};
use std::ffi::c_void;
use truce_rack_core::{
    buffer::AudioBuffer,
    error::{Error, Result},
    events::EventList,
    plugin::{Plugin, PluginCore, ProcessContext, ProcessStatus},
};

impl Plugin<f32> for Lv2Plugin {
    fn process(
        &mut self,
        buffer: &mut AudioBuffer<'_, f32>,
        events: &EventList,
        context: &mut ProcessContext<'_>,
    ) -> Result<ProcessStatus> {
        if !self.is_active() {
            return Err(Error::NotActivated);
        }
        self.drain_pending_controls();
        let frames = buffer.num_frames();
        if frames == 0 {
            return Ok(ProcessStatus::Continue);
        }

        // Validate the exact bus topology and mapping before touching any native port.
        let layout = self.active_layout.as_ref().ok_or(Error::NotActivated)?;
        let maximum_block = context
            .max_block_size
            .min(self.features.options.maximum_block());
        let planes = AudioPlanes::collect(buffer, layout, maximum_block)?;
        planes.validate_ports(&self.bindings.ports)?;

        // Build the input atom sequence (host transport + MIDI) and
        // reset the output.
        self.atoms
            .build_sequence(events, context.transport)
            .map_err(crate::atoms::AtomFault::into_error)?;
        self.atoms.prepare_output();

        let control_base = self.bindings.control_values.as_mut_ptr();
        let midi_in_ptr = self.atoms.input_ptr();

        // Connect each native port using its load-time raw-port → flattened-plane mapping.
        for port in &self.bindings.ports {
            let data: *mut c_void = match port.kind {
                PortKind::Audio => {
                    let Some(plane) = port.audio_plane else {
                        return Err(Error::Process {
                            format: crate::FORMAT,
                            fault: truce_rack_core::ProcessFault::InvalidAudioMapping,
                        });
                    };
                    if port.is_input {
                        planes.inputs[plane].cast::<c_void>()
                    } else {
                        planes.outputs[plane].cast::<c_void>()
                    }
                }
                PortKind::Control { .. } => {
                    let off = self.bindings.control_value_offset[port.index as usize];
                    if off == usize::MAX {
                        std::ptr::null_mut()
                    } else {
                        // SAFETY: `control_base` is the head of the
                        // `control_values` Vec; `off` was computed
                        // from its push order at load time.
                        unsafe { control_base.add(off).cast::<c_void>() }
                    }
                }
                PortKind::AtomSequence => {
                    if port.is_input {
                        midi_in_ptr
                    } else {
                        self.atoms.output_ptr(port.index)
                    }
                }
                PortKind::Other => std::ptr::null_mut(),
            };
            unsafe {
                lilv_sys::lilv_instance_connect_port(self.instance, port.index, data);
            }
        }

        unsafe {
            lilv_sys::lilv_instance_run(self.instance, frames as u32);
        }
        // LV2 exposes changing latency as a connected output control-port scalar, not a getter.
        // Copy that already-written value after run; this performs no FFI query, allocation, or lock.
        self.update_latency_snapshot();

        // Drain MIDI from the output atom-sequence port (if the
        // plugin connected one) into context.output_events.
        self.atoms
            .drain_output(context.output_events, frames as u32)
            .map_err(crate::atoms::AtomFault::into_error)?;

        Ok(ProcessStatus::Continue)
    }
}
