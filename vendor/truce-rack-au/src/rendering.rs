//! Preallocated Audio Unit render buffers and synchronous audio callbacks.

use crate::AuPlugin;
use crate::host_transport::{HostTransport, update_host_transport};
use objc2_audio_toolbox::{
    AudioComponentInstance, AudioUnitRender, AudioUnitRenderActionFlags, MusicDeviceMIDIEvent,
};
use objc2_core_audio_types::{
    AudioBuffer as CAAudioBuffer, AudioBufferList, AudioTimeStamp, AudioTimeStampFlags,
};
use std::ptr;
use truce_rack_core::buffer::AudioBuffer;
use truce_rack_core::error::{Error, ProcessFault, Result};
use truce_rack_core::events::EventList;
use truce_rack_core::plugin::{Plugin, PluginCore, ProcessContext, ProcessStatus};

/// Route one truce-rack [`truce_rack_core::events::Event`] into the AU via
/// `MusicDeviceMIDIEvent`. Non-MIDI events (param automation,
/// transport flags) are skipped — AU parameter automation goes
/// through `AudioUnitSetParameter` with a sample offset, and
/// transport flips are surfaced via the render-callback context.
pub(super) fn send_midi(unit: AudioComponentInstance, event: &truce_rack_core::events::Event) {
    use truce_rack_core::events::{EventBody, MidiData};
    let offset = event.sample_offset;
    let (status, d1, d2) = match event.body {
        EventBody::Midi(MidiData::NoteOn {
            channel,
            note,
            velocity,
        }) => (
            0x90 | u32::from(channel & 0x0F),
            u32::from(note & 0x7F),
            u32::from(velocity & 0x7F),
        ),
        EventBody::Midi(MidiData::NoteOff {
            channel,
            note,
            velocity,
        }) => (
            0x80 | u32::from(channel & 0x0F),
            u32::from(note & 0x7F),
            u32::from(velocity & 0x7F),
        ),
        EventBody::Midi(MidiData::ControlChange {
            channel,
            controller,
            value,
        }) => (
            0xB0 | u32::from(channel & 0x0F),
            u32::from(controller & 0x7F),
            u32::from(value & 0x7F),
        ),
        EventBody::Midi(MidiData::ProgramChange { channel, program }) => (
            0xC0 | u32::from(channel & 0x0F),
            u32::from(program & 0x7F),
            0,
        ),
        EventBody::Midi(MidiData::ChannelAftertouch { channel, pressure }) => (
            0xD0 | u32::from(channel & 0x0F),
            u32::from(pressure & 0x7F),
            0,
        ),
        EventBody::Midi(MidiData::PolyAftertouch {
            channel,
            note,
            pressure,
        }) => (
            0xA0 | u32::from(channel & 0x0F),
            u32::from(note & 0x7F),
            u32::from(pressure & 0x7F),
        ),
        EventBody::Midi(MidiData::PitchBend { channel, value }) => (
            0xE0 | u32::from(channel & 0x0F),
            u32::from(value & 0x7F),
            u32::from((value >> 7) & 0x7F),
        ),
        EventBody::Midi(MidiData::Raw { len, data }) if len >= 1 => {
            let s = u32::from(data[0]);
            let d1 = if len >= 2 { u32::from(data[1]) } else { 0 };
            let d2 = if len >= 3 { u32::from(data[2]) } else { 0 };
            (s, d1, d2)
        }
        _ => return,
    };
    // Non-MusicDevice / non-MIDIFX components return
    // kAudioUnitErr_InvalidProperty here; ignore.
    let _ = unsafe { MusicDeviceMIDIEvent(unit, status, d1, d2, offset) };
}

/// Stable native callback allocations and the preallocated buffers for one AU instance.
/// Registered refcons belong to this owner until native instance disposal completes.
#[derive(Default)]
pub(super) struct RenderState {
    pub(super) render_ctx: Option<Box<RenderContext>>,
    pub(super) output_buffer_storage: Vec<Vec<u8>>,
    pub(super) sample_time: f64,
    pub(super) host_transport: Option<Box<HostTransport>>,
    pub(super) max_block_size: usize,
}

impl RenderState {
    pub(super) fn prepare(
        &mut self,
        input_channels: &[u32],
        output_channels: &[u32],
        max_block_size: usize,
    ) -> Result<()> {
        let ranges = bus_plane_ranges(input_channels)
            .ok_or_else(|| Error::Other("Audio Unit input plane range overflow".into()))?;
        let plane_count = match ranges.last() {
            Some(range) => range
                .start
                .checked_add(range.len)
                .ok_or_else(|| Error::Other("Audio Unit input plane range overflow".into()))?,
            None => 0,
        };
        if let Some(ctx) = self.render_ctx.as_deref_mut() {
            ctx.input_planes.resize(plane_count, ptr::null());
            ctx.input_planes.fill(ptr::null());
            ctx.input_bus_ranges = ranges;
            ctx.frames = 0;
        } else {
            self.render_ctx = Some(Box::new(RenderContext {
                input_planes: vec![ptr::null(); plane_count],
                input_bus_ranges: ranges,
                frames: 0,
            }));
        }
        let mut host_transport = self
            .host_transport
            .take()
            .unwrap_or_else(|| Box::new(HostTransport::default()));
        *host_transport = HostTransport::default();
        self.host_transport = Some(host_transport);
        self.output_buffer_storage = output_channels
            .iter()
            .map(|&channels| alloc_audio_buffer_list(channels as usize))
            .collect();
        self.max_block_size = max_block_size;
        Ok(())
    }

    pub(super) fn suspend(&mut self) {
        self.finish_block();
        self.max_block_size = 0;
    }

    fn finish_block(&mut self) {
        // Native callback registrations remain installed while the unit is inactive.
        // Retain their addresses through disposal, but revoke every borrowed block pointer.
        if let Some(ctx) = self.render_ctx.as_deref_mut() {
            ctx.input_planes.fill(ptr::null());
            ctx.frames = 0;
        }
        if let Some(transport) = self.host_transport.as_deref_mut() {
            update_host_transport(transport, None, 0.0, 0.0);
        }
    }
}

/// Audio-thread input pointers refreshed by `process()` for synchronous native pulls.
pub(super) struct RenderContext {
    /// Flattened one pointer per channel in AU input-element order. NULL means silence.
    pub(super) input_planes: Vec<*const f32>,
    /// Flat plane range for each AU input element, indexed by the callback bus number.
    pub(super) input_bus_ranges: Vec<BusPlaneRange>,
    /// Readable frames in the current host block; zero outside synchronous rendering.
    pub(super) frames: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BusPlaneRange {
    pub(super) start: usize,
    pub(super) len: usize,
}

pub(super) fn bus_plane_ranges(channel_counts: &[u32]) -> Option<Vec<BusPlaneRange>> {
    let mut ranges = Vec::with_capacity(channel_counts.len());
    let mut start = 0_usize;
    for &channels in channel_counts {
        let len = usize::try_from(channels).ok()?;
        ranges.push(BusPlaneRange { start, len });
        start = start.checked_add(len)?;
    }
    Some(ranges)
}

pub(super) fn bus_plane_index(ranges: &[BusPlaneRange], bus: u32, channel: usize) -> Option<usize> {
    let range = ranges.get(usize::try_from(bus).ok()?)?;
    if channel >= range.len {
        return None;
    }
    range.start.checked_add(channel)
}

pub(super) unsafe extern "C-unwind" fn render_input_callback(
    in_ref_con: std::ptr::NonNull<std::ffi::c_void>,
    _io_action_flags: std::ptr::NonNull<AudioUnitRenderActionFlags>,
    _in_time_stamp: std::ptr::NonNull<AudioTimeStamp>,
    in_bus_number: u32,
    in_number_frames: u32,
    io_data: *mut AudioBufferList,
) -> i32 {
    if io_data.is_null() {
        return -1;
    }
    let ctx_ptr = in_ref_con.as_ptr().cast::<RenderContext>();
    let ctx = unsafe { &*ctx_ptr };
    let list = unsafe { &mut *io_data };
    let frames = usize::try_from(in_number_frames).unwrap_or(usize::MAX);
    let buffer_count = list.mNumberBuffers as usize;
    // The variable-length tail is reached via mBuffers as a [_; 1]
    // declared slice we extend via pointer arithmetic.
    let buffers_ptr: *mut CAAudioBuffer = list.mBuffers.as_mut_ptr();
    let bytes_u32 = frames
        .checked_mul(std::mem::size_of::<f32>())
        .and_then(|bytes| u32::try_from(bytes).ok());
    let range = usize::try_from(in_bus_number)
        .ok()
        .and_then(|index| ctx.input_bus_ranges.get(index));
    let mut failed = !matches!(range, Some(bus) if bus.len == buffer_count)
        || frames > ctx.frames
        || bytes_u32.is_none();
    for i in 0..buffer_count {
        let buffer = unsafe { &mut *buffers_ptr.add(i) };
        if let Some(plane_index) = bus_plane_index(&ctx.input_bus_ranges, in_bus_number, i)
            && let Some(&plane) = ctx.input_planes.get(plane_index)
            && !plane.is_null()
            && frames <= ctx.frames
            && let Some(bytes) = bytes_u32
        {
            buffer.mDataByteSize = bytes;
            buffer.mData = plane.cast::<std::ffi::c_void>().cast_mut();
            continue;
        }
        failed = true;
        // Silence only the storage the AU actually supplied. Never trust the requested frame
        // count to describe the capacity of this callback-owned buffer.
        if !buffer.mData.is_null() {
            let capacity = usize::try_from(buffer.mDataByteSize).unwrap_or(0);
            let silence_bytes = bytes_u32.map_or(capacity, |bytes| {
                capacity.min(usize::try_from(bytes).unwrap_or(0))
            });
            unsafe {
                std::ptr::write_bytes(buffer.mData.cast::<u8>(), 0, silence_bytes);
            }
        }
    }
    if failed { -1 } else { 0 }
}

/// Allocate enough bytes to hold an `AudioBufferList` with
/// `n_buffers` `AudioBuffer`s tail-included. Returns a `Vec<u8>`
/// large enough; the caller casts the data pointer.
pub(super) fn alloc_audio_buffer_list(n_buffers: usize) -> Vec<u8> {
    let n = n_buffers.max(1);
    let size =
        std::mem::size_of::<AudioBufferList>() + (n - 1) * std::mem::size_of::<CAAudioBuffer>();
    vec![0u8; size]
}

pub(super) fn process_buffer_matches_topology(
    buffer: &mut AudioBuffer<'_, f32>,
    input_channels: &[u32],
    output_channels: &[u32],
) -> bool {
    let frames = buffer.num_frames();
    if buffer.num_input_buses() != input_channels.len()
        || buffer.num_output_buses() != output_channels.len()
    {
        return false;
    }
    for (index, &expected) in input_channels.iter().enumerate() {
        let channels = buffer.bus_inputs(index);
        if usize::try_from(expected).ok() != Some(channels.len())
            || channels.iter().any(|plane| plane.len() < frames)
        {
            return false;
        }
    }
    for (index, &expected) in output_channels.iter().enumerate() {
        let channels = buffer.bus_outputs(index);
        if usize::try_from(expected).ok() != Some(channels.len())
            || channels.iter().any(|plane| plane.len() < frames)
        {
            return false;
        }
    }
    true
}

pub(super) fn refresh_render_inputs(
    render_ctx: Option<&mut RenderContext>,
    buffer: &AudioBuffer<'_, f32>,
) -> bool {
    let Some(ctx) = render_ctx else {
        return buffer.num_input_buses() == 0;
    };
    ctx.frames = 0;
    ctx.input_planes.fill(ptr::null());
    for (bus_index, range) in ctx.input_bus_ranges.iter().copied().enumerate() {
        for (channel_index, channel) in buffer.bus_inputs(bus_index).iter().enumerate() {
            let Some(plane_index) = range.start.checked_add(channel_index) else {
                return false;
            };
            let Some(slot) = ctx.input_planes.get_mut(plane_index) else {
                return false;
            };
            *slot = channel.as_ptr();
        }
    }
    ctx.frames = buffer.num_frames();
    true
}

pub(super) fn audio_timestamp(sample_time: f64) -> AudioTimeStamp {
    AudioTimeStamp {
        mSampleTime: sample_time,
        mHostTime: 0,
        mRateScalar: 1.0,
        mWordClockTime: 0,
        mSMPTETime: objc2_core_audio_types::SMPTETime {
            mSubframes: 0,
            mSubframeDivisor: 0,
            mCounter: 0,
            mType: objc2_core_audio_types::SMPTETimeType(0),
            mFlags: objc2_core_audio_types::SMPTETimeFlags(0),
            mHours: 0,
            mMinutes: 0,
            mSeconds: 0,
            mFrames: 0,
        },
        mFlags: AudioTimeStampFlags::SampleTimeValid,
        mReserved: 0,
    }
}

/// Render every output element into its matching checked host planes.
///
/// # Safety
/// The AU must be initialized and exclusively rendered. `buffer` has already been verified to
/// contain its declared block length in every output plane.
pub(super) unsafe fn render_output_buses(
    instance: AudioComponentInstance,
    output_channels: &[u32],
    storage: &mut [Vec<u8>],
    buffer: &mut AudioBuffer<'_, f32>,
    timestamp: &AudioTimeStamp,
) -> Result<()> {
    let Ok(frames) = u32::try_from(buffer.num_frames()) else {
        return Err(process_error(ProcessFault::InvalidBlockSize));
    };
    let Some(frame_bytes) =
        frames.checked_mul(u32::try_from(std::mem::size_of::<f32>()).unwrap_or(0))
    else {
        return Err(process_error(ProcessFault::InvalidBlockSize));
    };

    if storage.len() != output_channels.len() {
        return Err(process_error(ProcessFault::InvalidAudioMapping));
    }
    for (bus_index, audio_list) in storage.iter_mut().enumerate() {
        let Ok(output_bus_number) = u32::try_from(bus_index) else {
            return Err(process_error(ProcessFault::InvalidAudioMapping));
        };
        let Ok(channel_count) = usize::try_from(output_channels[bus_index]) else {
            return Err(process_error(ProcessFault::InvalidAudioMapping));
        };
        let bus_outputs = buffer.bus_outputs(bus_index);
        // SAFETY: the list is pre-sized for this bus, and each host plane passed validation.
        #[allow(clippy::cast_ptr_alignment)]
        let list_ptr = audio_list.as_mut_ptr().cast::<AudioBufferList>();
        unsafe {
            (*list_ptr).mNumberBuffers = output_channels[bus_index];
            let buffers_start: *mut CAAudioBuffer = (*list_ptr).mBuffers.as_mut_ptr();
            for (channel_index, plane) in bus_outputs.iter_mut().take(channel_count).enumerate() {
                let output_buffer = &mut *buffers_start.add(channel_index);
                output_buffer.mNumberChannels = 1;
                output_buffer.mDataByteSize = frame_bytes;
                output_buffer.mData = plane.as_mut_ptr().cast();
            }
        }
        let mut flags = AudioUnitRenderActionFlags::empty();
        let status = unsafe {
            AudioUnitRender(
                instance,
                &raw mut flags,
                ptr::NonNull::from(timestamp),
                output_bus_number,
                frames,
                ptr::NonNull::new_unchecked(list_ptr),
            )
        };
        if status != 0 {
            return Err(process_error(ProcessFault::NativeStatus(i64::from(status))));
        }
    }
    Ok(())
}

fn process_error(fault: ProcessFault) -> Error {
    Error::Process {
        format: crate::FORMAT,
        fault,
    }
}

impl Plugin<f32> for AuPlugin {
    fn process(
        &mut self,
        buffer: &mut AudioBuffer<'_, f32>,
        events: &EventList,
        context: &mut ProcessContext<'_>,
    ) -> Result<ProcessStatus> {
        if !self.is_active() {
            return Err(Error::NotActivated);
        }
        if buffer.num_frames() > self.render.max_block_size {
            return Err(process_error(ProcessFault::InvalidBlockSize));
        }
        if !process_buffer_matches_topology(
            buffer,
            &self.input_bus_channels,
            &self.output_bus_channels,
        ) {
            return Err(process_error(ProcessFault::InvalidAudioMapping));
        }
        let frames = buffer.num_frames();
        let sample_time = self.render.sample_time;
        if let Some(ht) = self.render.host_transport.as_deref_mut() {
            update_host_transport(ht, context.transport, context.sample_rate, sample_time);
        }
        for event in events {
            send_midi(self.instance, event);
        }
        if !refresh_render_inputs(self.render.render_ctx.as_deref_mut(), buffer) {
            self.render.finish_block();
            return Err(process_error(ProcessFault::InvalidAudioMapping));
        }
        let timestamp = audio_timestamp(sample_time);
        // SAFETY: activation succeeded, bus topology was checked, and every plane is long enough.
        let rendered = unsafe {
            render_output_buses(
                self.instance,
                &self.output_bus_channels,
                &mut self.render.output_buffer_storage,
                buffer,
                &timestamp,
            )
        };
        self.render.finish_block();
        #[allow(clippy::cast_precision_loss)]
        let frames_f = frames as f64;
        self.render.sample_time += frames_f;
        rendered?;
        Ok(ProcessStatus::Continue)
    }
}

#[cfg(test)]
mod tests;
