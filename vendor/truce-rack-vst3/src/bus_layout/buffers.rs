//! Build checked stack-backed native bus views for one audio call.

use super::{MAX_BUSES, MAX_CHANNELS, NativeLayout};
use std::ptr;
use truce_rack_core::{
    buffer::AudioBuffer,
    error::{Error, ProcessFault, Result},
};
use vst3::Steinberg::Vst::{AudioBusBuffers, AudioBusBuffers__type0};

fn empty_bus() -> AudioBusBuffers {
    AudioBusBuffers {
        numChannels: 0,
        silenceFlags: 0,
        __field0: AudioBusBuffers__type0 {
            channelBuffers32: ptr::null_mut(),
        },
    }
}

/// Borrowed sample pointers live only for the synchronous closure. Stack arrays
/// cover the host's channel/bus limits and replace per-block channel Vec allocation.
pub(crate) fn with_buffers<T>(
    buffer: &mut AudioBuffer<'_, f32>,
    native_layout: &NativeLayout,
    process: impl FnOnce(&mut [AudioBusBuffers], &mut [AudioBusBuffers]) -> T,
) -> Result<T> {
    let layout = &native_layout.layout;
    if layout.inputs.len() > MAX_BUSES
        || layout.outputs.len() > MAX_BUSES
        || buffer.num_input_buses() != layout.inputs.len()
        || buffer.num_output_buses() != layout.outputs.len()
        || buffer.total_input_channels() > MAX_CHANNELS
        || buffer.total_output_channels() > MAX_CHANNELS
    {
        return Err(invalid_mapping());
    }
    let mut input_ptrs = [ptr::null_mut(); MAX_CHANNELS];
    let mut output_ptrs = [ptr::null_mut(); MAX_CHANNELS];
    let mut inputs = std::array::from_fn::<_, MAX_BUSES, _>(|_| empty_bus());
    let mut outputs = std::array::from_fn::<_, MAX_BUSES, _>(|_| empty_bus());
    let frames = buffer.num_frames();
    let mut cursor = 0;
    for (index, expected) in layout.inputs.iter().enumerate() {
        let channels = buffer.try_bus_inputs(index).ok_or_else(invalid_mapping)?;
        if channels.len() != expected.channels.count() as usize
            || channels.iter().any(|channel| channel.len() < frames)
        {
            return Err(invalid_mapping());
        }
        for (slot, channel) in input_ptrs[cursor..].iter_mut().zip(channels) {
            *slot = channel.as_ptr().cast_mut();
        }
        inputs[index].numChannels = i32::try_from(channels.len()).unwrap_or_default();
        inputs[index].__field0.channelBuffers32 = input_ptrs[cursor..].as_mut_ptr();
        cursor += channels.len();
    }
    cursor = 0;
    for (index, expected) in layout.outputs.iter().enumerate() {
        let channels = buffer.try_bus_outputs(index).ok_or_else(invalid_mapping)?;
        if channels.len() != expected.channels.count() as usize
            || channels.iter().any(|channel| channel.len() < frames)
        {
            return Err(invalid_mapping());
        }
        for (slot, channel) in output_ptrs[cursor..].iter_mut().zip(channels.iter_mut()) {
            *slot = channel.as_mut_ptr();
        }
        outputs[index].numChannels = i32::try_from(channels.len()).unwrap_or_default();
        outputs[index].__field0.channelBuffers32 = output_ptrs[cursor..].as_mut_ptr();
        cursor += channels.len();
    }
    Ok(process(
        &mut inputs[..layout.inputs.len()],
        &mut outputs[..layout.outputs.len()],
    ))
}

fn invalid_mapping() -> Error {
    Error::Process {
        format: crate::FORMAT,
        fault: ProcessFault::InvalidAudioMapping,
    }
}
