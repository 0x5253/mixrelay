//! Native PCM negotiation, format readback and activation rollback.

use std::ptr;

use crate::bus_topology::{
    read_channel_layout_tag, read_stream_format, read_u32_property, selected_channel_config_matches,
};
use objc2_audio_toolbox::{
    AudioComponentInstance, AudioUnitElement, AudioUnitScope, AudioUnitSetProperty,
    kAudioUnitProperty_ElementCount, kAudioUnitProperty_MaximumFramesPerSlice,
    kAudioUnitProperty_StreamFormat, kAudioUnitScope_Global, kAudioUnitScope_Input,
    kAudioUnitScope_Output,
};
use objc2_core_audio_types::{
    AudioStreamBasicDescription, kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved,
    kAudioFormatFlagIsPacked, kAudioFormatLinearPCM,
};
use truce_rack_core::bus::BusLayout;
use truce_rack_core::error::{Error, Result};

#[derive(Clone, Copy)]
pub(super) struct NativeBusFormat {
    pub(super) scope: AudioUnitScope,
    pub(super) element: AudioUnitElement,
    pub(super) format: AudioStreamBasicDescription,
}

pub(super) fn planar_f32_format(sample_rate: f64, channels: u32) -> AudioStreamBasicDescription {
    AudioStreamBasicDescription {
        mSampleRate: sample_rate,
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagIsFloat
            | kAudioFormatFlagIsPacked
            | kAudioFormatFlagIsNonInterleaved,
        mBytesPerPacket: 4,
        mFramesPerPacket: 1,
        mBytesPerFrame: 4,
        mChannelsPerFrame: channels,
        mBitsPerChannel: 32,
        mReserved: 0,
    }
}

pub(super) fn render_format_matches(
    actual: &AudioStreamBasicDescription,
    expected: &AudioStreamBasicDescription,
) -> bool {
    actual.mSampleRate.to_bits() == expected.mSampleRate.to_bits()
        && actual.mFormatID == expected.mFormatID
        && actual.mFormatFlags == expected.mFormatFlags
        && actual.mBytesPerPacket == expected.mBytesPerPacket
        && actual.mFramesPerPacket == expected.mFramesPerPacket
        && actual.mBytesPerFrame == expected.mBytesPerFrame
        && actual.mChannelsPerFrame == expected.mChannelsPerFrame
        && actual.mBitsPerChannel == expected.mBitsPerChannel
}

/// Set one AU element's native stream format.
///
/// # Safety
/// `unit` must be a live Audio Unit instance, and the caller must serialize this mutation with
/// all other native access to the instance.
pub(super) unsafe fn set_stream_format(
    unit: AudioComponentInstance,
    scope: AudioUnitScope,
    element: AudioUnitElement,
    format: &AudioStreamBasicDescription,
) -> i32 {
    unsafe {
        AudioUnitSetProperty(
            unit,
            kAudioUnitProperty_StreamFormat,
            scope,
            element,
            ptr::from_ref(format).cast(),
            u32::try_from(std::mem::size_of::<AudioStreamBasicDescription>()).unwrap_or(0),
        )
    }
}

/// Restore previously observed formats after a failed activation attempt.
///
/// # Safety
/// `unit` must be a live Audio Unit instance, and the caller must serialize native access to it.
pub(super) unsafe fn restore_native_formats(
    unit: AudioComponentInstance,
    previous: &[NativeBusFormat],
) -> Vec<String> {
    let mut failures = Vec::new();
    for item in previous {
        let status = unsafe { set_stream_format(unit, item.scope, item.element, &item.format) };
        if status != 0 {
            failures.push(format!(
                "restore stream format scope {} element {} returned OSStatus {status}",
                item.scope, item.element
            ));
            continue;
        }
        match unsafe { read_stream_format(unit, item.scope, item.element) } {
            Ok(restored) if restored == item.format => {}
            Ok(_) => failures.push(format!(
                "restore stream format scope {} element {} did not reproduce the observed format",
                item.scope, item.element
            )),
            Err(error) => failures.push(format!(
                "verify restored stream format scope {} element {} failed: {error}",
                item.scope, item.element
            )),
        }
    }
    failures
}

/// Build a failure while restoring the AU's pre-activation stream formats.
///
/// # Safety
/// `unit` must be a live Audio Unit instance and native access must be serialized by the caller.
pub(super) unsafe fn activation_error(
    unit: AudioComponentInstance,
    previous: &[NativeBusFormat],
    cause: Error,
) -> Error {
    let failures = unsafe { restore_native_formats(unit, previous) };
    if failures.is_empty() {
        cause
    } else {
        Error::Other(format!(
            "{cause}; additionally, native format rollback was incomplete: {}",
            failures.join("; ")
        ))
    }
}

/// Snapshot every native bus format before activation mutates any element.
///
/// # Safety
/// `unit` must be live and exclusively accessed by the caller.
pub(super) unsafe fn capture_native_formats(
    unit: AudioComponentInstance,
    input_channels: &[u32],
    output_channels: &[u32],
) -> Result<Vec<NativeBusFormat>> {
    for (scope, expected) in [
        (kAudioUnitScope_Input, input_channels.len()),
        (kAudioUnitScope_Output, output_channels.len()),
    ] {
        let count = unsafe { read_u32_property(unit, kAudioUnitProperty_ElementCount, scope, 0)? };
        if usize::try_from(count).ok() != Some(expected) {
            return Err(Error::Other(
                "Audio Unit bus counts changed after topology discovery".into(),
            ));
        }
    }
    let mut formats = Vec::with_capacity(input_channels.len() + output_channels.len());
    for (scope, channels) in [
        (kAudioUnitScope_Input, input_channels),
        (kAudioUnitScope_Output, output_channels),
    ] {
        for index in 0..channels.len() {
            let element = u32::try_from(index)
                .map_err(|_| Error::Other("Audio Unit bus index overflow".into()))?;
            formats.push(NativeBusFormat {
                scope,
                element,
                // SAFETY: the instance is live and serialized by activation.
                format: unsafe { read_stream_format(unit, scope, element)? },
            });
        }
    }
    Ok(formats)
}

/// Apply and read back planar f32 formats and the observed bus topology.
///
/// # Safety
/// `unit` must be live and exclusively accessed by the caller. Any partial mutation must be
/// rolled back using the pre-activation snapshot if this returns an error.
pub(super) unsafe fn apply_and_verify_native_formats(
    unit: AudioComponentInstance,
    input_channels: &[u32],
    output_channels: &[u32],
    layout: &BusLayout,
    sample_rate: f64,
) -> std::result::Result<(), String> {
    for (scope, channels) in [
        (kAudioUnitScope_Input, input_channels),
        (kAudioUnitScope_Output, output_channels),
    ] {
        for (index, &count) in channels.iter().enumerate() {
            let element = u32::try_from(index).map_err(|_| "Audio Unit bus index overflow")?;
            let expected = planar_f32_format(sample_rate, count);
            let status = unsafe { set_stream_format(unit, scope, element, &expected) };
            if status != 0 {
                return Err(format!(
                    "AudioUnitSetProperty(StreamFormat, scope {scope}, element {element}) failed: OSStatus {status}"
                ));
            }
        }
    }
    unsafe { verify_native_formats(unit, layout, sample_rate) }
}

/// Read back native counts, PCM formats and channel grouping without mutating the instance.
///
/// # Safety
/// The live instance is exclusively accessed on its creator thread, outside rendering.
pub(super) unsafe fn verify_native_formats(
    unit: AudioComponentInstance,
    layout: &BusLayout,
    sample_rate: f64,
) -> std::result::Result<(), String> {
    for (scope, buses) in [
        (kAudioUnitScope_Input, layout.inputs.as_slice()),
        (kAudioUnitScope_Output, layout.outputs.as_slice()),
    ] {
        let count = unsafe { read_u32_property(unit, kAudioUnitProperty_ElementCount, scope, 0) }
            .map_err(|error| format!("verify Audio Unit bus count failed: {error}"))?;
        if usize::try_from(count).ok() != Some(buses.len()) {
            return Err(format!("Audio Unit bus count changed on scope {scope}"));
        }
        for (index, bus) in buses.iter().enumerate() {
            let element = u32::try_from(index).map_err(|_| "Audio Unit bus index overflow")?;
            let expected = planar_f32_format(sample_rate, bus.channels.count());
            let actual = unsafe { read_stream_format(unit, scope, element) }
                .map_err(|error| format!("verify Audio Unit stream format failed: {error}"))?;
            if !render_format_matches(&actual, &expected) {
                return Err(format!(
                    "Audio Unit did not accept planar f32 on scope {scope}, element {element}"
                ));
            }
            let layout_tag = unsafe { read_channel_layout_tag(unit, scope, element) };
            if !selected_channel_config_matches(bus.channels, actual.mChannelsPerFrame, layout_tag)
            {
                return Err(format!(
                    "Audio Unit channel layout changed on scope {scope}, element {element}"
                ));
            }
        }
    }
    Ok(())
}

/// Set the AU's maximum render quantum.
///
/// # Safety
/// `unit` must be live and exclusively accessed by the caller.
pub(super) unsafe fn set_maximum_frames(
    unit: AudioComponentInstance,
    max_frames: u32,
) -> std::result::Result<(), String> {
    let status = unsafe {
        AudioUnitSetProperty(
            unit,
            kAudioUnitProperty_MaximumFramesPerSlice,
            kAudioUnitScope_Global,
            0,
            (&raw const max_frames).cast(),
            u32::try_from(std::mem::size_of::<u32>()).unwrap_or(0),
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(format!(
            "AudioUnitSetProperty(MaximumFramesPerSlice) failed: OSStatus {status}"
        ))
    }
}
