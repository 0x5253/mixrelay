//! Prepared audio pointers and synchronous native DSP calls.

use crate::events::{OutputEventsSink, build_clap_transport, map_clap_status};
use crate::host::HostContext;
use crate::parameters::commit_processed_parameter_changes;
use crate::{ClapPlugin, FORMAT};
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::{clap_event_transport, clap_input_events, clap_output_events};
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
use std::ptr;
use truce_rack_core::buffer::AudioBuffer;
use truce_rack_core::bus::BusLayout;
use truce_rack_core::error::{Error, ProcessFault, Result};
use truce_rack_core::events::EventList;
use truce_rack_core::plugin::{Plugin, PluginCore, ProcessContext, ProcessStatus};
use truce_rack_core::wrapper::run_audio_block_with;

/// Per-bus channel pointers allocated for the accepted layout during activation.
/// Values are filled for one synchronous block and reset before that block returns.
pub(super) struct ProcessChannelPointers {
    pub(super) input_channels: Vec<Vec<*mut f32>>,
    pub(super) output_channels: Vec<Vec<*mut f32>>,
    pub(super) input_buffers: Vec<clap_audio_buffer>,
    pub(super) output_buffers: Vec<clap_audio_buffer>,
    pub(super) input_count: u32,
    pub(super) output_count: u32,
}

impl ProcessChannelPointers {
    pub(super) fn for_layout(layout: &BusLayout) -> Result<Self> {
        let (input_channels, input_buffers, input_count) = bus_pointer_slots(&layout.inputs)?;
        let (output_channels, output_buffers, output_count) = bus_pointer_slots(&layout.outputs)?;
        Ok(Self {
            input_channels,
            output_channels,
            input_buffers,
            output_buffers,
            input_count,
            output_count,
        })
    }

    pub(super) fn matches_buffer(&self, buffer: &mut AudioBuffer<'_, f32>) -> bool {
        if self.input_channels.len() != buffer.num_input_buses()
            || self.output_channels.len() != buffer.num_output_buses()
        {
            return false;
        }
        self.input_channels
            .iter()
            .enumerate()
            .all(|(index, channels)| buffer.bus_inputs(index).len() == channels.len())
            && self
                .output_channels
                .iter()
                .enumerate()
                .all(|(index, channels)| buffer.bus_outputs(index).len() == channels.len())
    }

    pub(super) fn fill_from(&mut self, buffer: &mut AudioBuffer<'_, f32>) -> bool {
        self.clear();
        if !self.matches_buffer(buffer) {
            return false;
        }

        for (bus_index, (pointer_slots, audio_buffer)) in self
            .input_channels
            .iter_mut()
            .zip(&mut self.input_buffers)
            .enumerate()
        {
            for (pointer, channel) in pointer_slots.iter_mut().zip(buffer.bus_inputs(bus_index)) {
                *pointer = channel.as_ptr().cast_mut();
            }
            audio_buffer.data32 = if pointer_slots.is_empty() {
                ptr::null_mut()
            } else {
                pointer_slots.as_mut_ptr()
            };
        }

        for (bus_index, (pointer_slots, audio_buffer)) in self
            .output_channels
            .iter_mut()
            .zip(&mut self.output_buffers)
            .enumerate()
        {
            for (pointer, channel) in pointer_slots
                .iter_mut()
                .zip(buffer.bus_outputs(bus_index).iter_mut())
            {
                *pointer = (*channel).as_mut_ptr();
            }
            audio_buffer.data32 = if pointer_slots.is_empty() {
                ptr::null_mut()
            } else {
                pointer_slots.as_mut_ptr()
            };
        }
        true
    }

    pub(super) fn clear(&mut self) {
        for (channels, audio_buffer) in self
            .input_channels
            .iter_mut()
            .zip(&mut self.input_buffers)
            .chain(
                self.output_channels
                    .iter_mut()
                    .zip(&mut self.output_buffers),
            )
        {
            channels.fill(ptr::null_mut());
            audio_buffer.data32 = ptr::null_mut();
        }
    }
}

pub(super) type BusPointerSlots = (Vec<Vec<*mut f32>>, Vec<clap_audio_buffer>, u32);

pub(super) fn bus_pointer_slots(buses: &[truce_rack_core::bus::Bus]) -> Result<BusPointerSlots> {
    if buses.len() > 64 {
        return Err(Error::Other("CLAP layout exceeds 64 audio buses".into()));
    }
    let bus_count = u32::try_from(buses.len())
        .map_err(|_| Error::Other("CLAP audio bus count exceeds u32".into()))?;
    let mut channels_by_bus = Vec::new();
    channels_by_bus
        .try_reserve_exact(buses.len())
        .map_err(|error| Error::Other(format!("cannot reserve CLAP bus pointers: {error}")))?;
    let mut audio_buffers = Vec::new();
    audio_buffers
        .try_reserve_exact(buses.len())
        .map_err(|error| Error::Other(format!("cannot reserve CLAP audio buffers: {error}")))?;

    let mut total_channels = 0_usize;
    for bus in buses {
        let channel_count = usize::try_from(bus.channels.count())
            .map_err(|_| Error::Other("CLAP bus channel count exceeds usize".into()))?;
        total_channels = total_channels
            .checked_add(channel_count)
            .ok_or_else(|| Error::Other("CLAP total channel count overflow".into()))?;
        if channel_count == 0 || total_channels > 64 {
            return Err(Error::Other(
                "CLAP direction must contain at most 64 total channels".into(),
            ));
        }
        let channel_count_u32 = u32::try_from(channel_count)
            .map_err(|_| Error::Other("CLAP bus channel count exceeds u32".into()))?;
        let mut channel_pointers = Vec::new();
        channel_pointers
            .try_reserve_exact(channel_count)
            .map_err(|error| {
                Error::Other(format!("cannot reserve CLAP channel pointers: {error}"))
            })?;
        channel_pointers.resize(channel_count, ptr::null_mut());
        channels_by_bus.push(channel_pointers);
        audio_buffers.push(clap_audio_buffer {
            data32: ptr::null_mut(),
            data64: ptr::null_mut(),
            channel_count: channel_count_u32,
            latency: 0,
            constant_mask: 0,
        });
    }
    Ok((channels_by_bus, audio_buffers, bus_count))
}

pub(super) struct ResetProcessPointers<'a> {
    pub(super) pointers: &'a mut ProcessChannelPointers,
}

impl Drop for ResetProcessPointers<'_> {
    fn drop(&mut self) {
        self.pointers.clear();
    }
}

pub(super) struct NativeProcessTarget<'a> {
    pub(super) plugin: *const clap_plugin,
    pub(super) host_context: &'a HostContext,
    pub(super) steady_time: i64,
    pub(super) pointers: &'a mut ProcessChannelPointers,
}

/// Build the borrowed CLAP block records and synchronously call the native processor.
pub(super) fn run_native_process_block(
    target: NativeProcessTarget<'_>,
    buffer: &mut AudioBuffer<'_, f32>,
    transport_event: Option<clap_event_transport>,
    input_events: &clap_input_events,
    output_events: &clap_output_events,
) -> i32 {
    let NativeProcessTarget {
        plugin,
        host_context,
        steady_time,
        pointers,
    } = target;
    if !pointers.fill_from(buffer) {
        return CLAP_PROCESS_ERROR;
    }
    let pointer_reset = ResetProcessPointers { pointers };
    let num_frames = buffer.num_frames();
    let process = clap_process {
        steady_time,
        frames_count: u32::try_from(num_frames).unwrap_or(u32::MAX),
        transport: transport_event
            .as_ref()
            .map_or(ptr::null(), std::ptr::from_ref),
        audio_inputs: if pointer_reset.pointers.input_buffers.is_empty() {
            ptr::null()
        } else {
            pointer_reset.pointers.input_buffers.as_ptr()
        },
        audio_outputs: if pointer_reset.pointers.output_buffers.is_empty() {
            ptr::null_mut()
        } else {
            pointer_reset.pointers.output_buffers.as_mut_ptr()
        },
        audio_inputs_count: pointer_reset.pointers.input_count,
        audio_outputs_count: pointer_reset.pointers.output_count,
        in_events: input_events,
        out_events: output_events,
    };

    let process_ptr = unsafe { (*plugin).process };
    match process_ptr {
        Some(process_fn) => {
            // `request_process` is coalesced until a real transport block reaches this native
            // call. It does not synthesize processing while the graph is idle/OFF.
            let _requested_process = host_context.take_process_request();
            let _audio_thread = host_context.audio_thread_scope();
            run_audio_block_with::<ClapPlugin, i32>(FORMAT, CLAP_PROCESS_ERROR, || unsafe {
                process_fn(plugin, &raw const process)
            })
        }
        None => CLAP_PROCESS_ERROR,
    }
}

impl ClapPlugin {
    /// Borrow an engine buffer as CLAP's per-block audio records and make one native process call.
    pub(super) fn call_process_block(
        &mut self,
        buffer: &mut AudioBuffer<'_, f32>,
        transport_event: Option<clap_event_transport>,
        input_events: &clap_input_events,
        output_events: &clap_output_events,
    ) -> i32 {
        let Some(pointers) = self.processing.process_pointers.as_mut() else {
            return CLAP_PROCESS_ERROR;
        };
        run_native_process_block(
            NativeProcessTarget {
                plugin: self.plugin,
                host_context: &self.host_context,
                steady_time: self.processing.steady_time,
                pointers,
            },
            buffer,
            transport_event,
            input_events,
            output_events,
        )
    }
}

impl Plugin<f32> for ClapPlugin {
    fn process(
        &mut self,
        buffer: &mut AudioBuffer<'_, f32>,
        events: &EventList,
        context: &mut ProcessContext<'_>,
    ) -> Result<ProcessStatus> {
        if !self.is_active() {
            return Err(Error::NotActivated);
        }
        if !self
            .processing
            .process_pointers
            .as_ref()
            .is_some_and(|pointers| pointers.matches_buffer(buffer))
        {
            // A mismatched host buffer must not reach start_processing or process, and the
            // real-time rejection path deliberately uses no allocated error message.
            return Ok(ProcessStatus::Error);
        }
        let Some(converted) = self.processing.input_events.as_mut() else {
            return Ok(ProcessStatus::Error);
        };
        if !converted.fill_for_process(events, &self.parameters.ids) {
            return Ok(ProcessStatus::Error);
        }
        // The C record borrows prepared storage that remains untouched through the native call.
        let input_events = converted.as_clap();
        if !self.processing.started_processing {
            let start = unsafe { (*self.plugin).start_processing }.ok_or(Error::Process {
                format: FORMAT,
                fault: ProcessFault::MissingCallback,
            })?;
            let _audio_thread = self.host_context.audio_thread_scope();
            if !unsafe { start(self.plugin) } {
                return Err(Error::Process {
                    format: FORMAT,
                    fault: ProcessFault::NativeStatus(i64::from(CLAP_PROCESS_ERROR)),
                });
            }
            self.processing.started_processing = true;
        }

        // Translate the host transport snapshot up front so its
        // backing struct lives for the whole process call.
        let transport_event = context
            .transport
            .map(|t| build_clap_transport(&t, context.sample_rate));

        let mut sink = OutputEventsSink::new(Some(context.output_events));
        let output_events = sink.as_clap();

        let num_frames = buffer.num_frames();
        let status =
            self.call_process_block(buffer, transport_event, &input_events, &output_events);

        commit_processed_parameter_changes(&mut self.parameters.ids, status);

        self.processing.steady_time = self
            .processing
            .steady_time
            .saturating_add(i64::try_from(num_frames).unwrap_or(0));

        Ok(map_clap_status(status))
    }
}
