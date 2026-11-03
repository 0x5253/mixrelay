//! Latency notification invalidation and seconds-to-frame conversion.

use crate::latency_signal;
use crate::latency_signal::LatencySignalLease;
use objc2_audio_toolbox::{
    AudioComponentInstance, AudioUnit, AudioUnitElement, AudioUnitGetProperty, AudioUnitPropertyID,
    AudioUnitScope, kAudioUnitErr_InvalidProperty, kAudioUnitProperty_Latency,
    kAudioUnitScope_Global,
};
use std::ptr;

/// Convert an AU's reported latency in seconds to sample frames.
///
/// AU reports processing latency as `Float64` seconds. Rounding up ensures the host
/// does not under-compensate a fractional-frame report. Invalid rates, negative or
/// non-finite latency, and values that cannot fit the host's frame counter stay unknown.
pub(super) fn latency_frames_from_seconds(seconds: f64, sample_rate: f64) -> Option<u32> {
    if !seconds.is_finite() || seconds < 0.0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return None;
    }
    let frames = seconds * sample_rate;
    if !frames.is_finite() {
        return None;
    }
    // AU stores seconds as Float64. For a value derived from an integer frame count, rounding
    // the seconds and multiplying back can land a couple representable steps above that integer.
    // Bound that round-trip error by half an input-seconds ULP scaled to frames plus half a
    // product ULP; only snap inside this arithmetic bound. Genuine fractional-frame reports ceil.
    let round_trip_error =
        ((sample_rate * float_spacing(seconds) + float_spacing(frames)) * 0.5).next_up();
    let nearest_integer = frames.round();
    let rounded_frames = if (frames - nearest_integer).abs() <= round_trip_error {
        nearest_integer
    } else {
        frames.ceil()
    };
    if rounded_frames > f64::from(u32::MAX) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(rounded_frames as u32)
}

/// Distance to the adjacent representable Float64 value for finite nonnegative input.
pub(super) fn float_spacing(value: f64) -> f64 {
    let upward = value.next_up() - value;
    let downward = value - value.next_down();
    if upward.is_finite() {
        upward.max(downward)
    } else {
        downward
    }
}

/// Read the AU latency property on the serialized plug-in lifecycle thread.
///
/// Apple's AU contract defines an unimplemented latency property as zero latency;
/// other property-query failures are not reliable zeroes and remain unknown.
///
/// # Safety
/// `unit` must be a live initialized AU instance, and the caller must serialize this property
/// query with processing and other native calls on that instance.
pub(super) unsafe fn query_latency_frames(
    unit: AudioComponentInstance,
    sample_rate: f64,
) -> Option<u32> {
    let mut seconds = 0.0_f64;
    let mut size = u32::try_from(std::mem::size_of::<f64>()).ok()?;
    // SAFETY: `unit` is the live initialized instance owned by `AuPlugin`; `seconds` and `size`
    // are valid writable storage of the exact property type/size for the duration of the call.
    let status = unsafe {
        AudioUnitGetProperty(
            unit,
            kAudioUnitProperty_Latency,
            kAudioUnitScope_Global,
            0,
            ptr::NonNull::new((&raw mut seconds).cast::<std::ffi::c_void>())?,
            ptr::NonNull::new(&raw mut size)?,
        )
    };
    if status == kAudioUnitErr_InvalidProperty {
        return Some(0);
    }
    if status != 0 || usize::try_from(size).ok()? != std::mem::size_of::<f64>() {
        return None;
    }
    latency_frames_from_seconds(seconds, sample_rate)
}

/// AU property-listener callback. It deliberately touches no native state: all
/// latency queries happen later from `service_main_thread` on the serialized owner.
pub(super) unsafe extern "C-unwind" fn latency_property_changed(
    user_data: std::ptr::NonNull<std::ffi::c_void>,
    _unit: AudioUnit,
    _property_id: AudioUnitPropertyID,
    _scope: AudioUnitScope,
    _element: AudioUnitElement,
) {
    latency_signal::signal(user_data);
}

pub(super) fn latency_snapshot(
    cached: Option<u32>,
    signal: Option<&LatencySignalLease<'_>>,
) -> Option<u32> {
    match signal {
        Some(lease) if !lease.is_dirty() => cached,
        _ => None,
    }
}

pub(super) fn take_latency_invalidation(signal: Option<&LatencySignalLease<'_>>) -> bool {
    signal.is_some_and(LatencySignalLease::take_invalidation)
}

pub(super) fn with_latency_invalidation<T>(
    cached: &mut Option<u32>,
    signal: Option<&LatencySignalLease<'_>>,
    mutation: impl FnOnce() -> T,
) -> T {
    *cached = None;
    if let Some(signal) = signal {
        signal.mark_dirty();
    }
    mutation()
}

pub(super) fn register_listener(
    instance: AudioComponentInstance,
) -> Option<LatencySignalLease<'static>> {
    crate::latency_signal::acquire().and_then(|signal| {
        // SAFETY: load owns the live instance and the generation-checked lease owns the refcon.
        let registered = unsafe {
            objc2_audio_toolbox::AudioUnitAddPropertyListener(
                instance,
                kAudioUnitProperty_Latency,
                Some(latency_property_changed),
                signal.user_data(),
            ) == 0
        };
        registered.then_some(signal)
    })
}
