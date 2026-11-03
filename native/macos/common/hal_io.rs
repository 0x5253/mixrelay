//! HAL transport lifecycle and shared clock publication.
//!
//! Control drains callbacks before binding, rearming or retiring a mapping.
//! PCM belongs to the HAL core's client queues; shared rings retain wire lifecycle metadata.
use crate::endpoint_transport::{
    EndpointTransport, EndpointTransportConfig as EndpointConfig,
    mr_endpoint_transport_begin_clock_transition, mr_endpoint_transport_capture_buffer,
    mr_endpoint_transport_clock_state, mr_endpoint_transport_config,
    mr_endpoint_transport_render_buffer, mr_endpoint_transport_requires_replacement,
};
use crate::shared_audio_buffer::{self as buffer, SharedAudioBuffer};
use std::{
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicPtr, AtomicU32, Ordering},
};

const MAX_EXACT_FRAME: f64 = 281_474_976_710_655.0;
#[repr(C)]
pub struct HalIoContext {
    render_activity: AtomicU32,
    clock_epoch: u64,
    clock_sample_rate: u32,
    clock_running: bool,
    transport: AtomicPtr<EndpointTransport>,
    render_buffer: AtomicPtr<SharedAudioBuffer>,
    capture_buffer: AtomicPtr<SharedAudioBuffer>,
    pub render_channels: u32,
    pub capture_channels: u32,
}
const _: () = assert!(std::mem::size_of::<HalIoContext>() == 56);
const _: () = assert!(std::mem::align_of::<HalIoContext>() == 8);
const _: () = assert!(std::mem::offset_of!(HalIoContext, transport) == 24);
const _: () = assert!(std::mem::offset_of!(HalIoContext, render_channels) == 48);
const _: () = assert!(cfg!(target_has_atomic = "ptr") && cfg!(target_has_atomic = "32"));

impl HalIoContext {
    /// Constructs an unattached control-owned context without allocation.
    #[must_use]
    pub fn new(render: u32, capture: u32) -> Option<Self> {
        valid_channels(render, capture).then(|| Self {
            render_activity: AtomicU32::new(1),
            clock_epoch: 0,
            clock_sample_rate: 0,
            clock_running: false,
            transport: AtomicPtr::new(ptr::null_mut()),
            render_buffer: AtomicPtr::new(ptr::null_mut()),
            capture_buffer: AtomicPtr::new(ptr::null_mut()),
            render_channels: render,
            capture_channels: capture,
        })
    }

    /// Prepares a replacement before binding. Does not alias its mapping or
    /// publish lifecycle changes into the currently attached transport.
    pub fn inherit_lifecycle(&mut self, previous: &Self) {
        self.clock_epoch = previous.clock_epoch;
        self.clock_sample_rate = previous.clock_sample_rate;
        self.clock_running = previous.clock_running;
        self.render_activity.store(
            previous.render_activity.load(Ordering::Acquire),
            Ordering::Relaxed,
        );
    }

    fn capture(&self) -> *mut SharedAudioBuffer {
        self.capture_buffer.load(Ordering::Acquire)
    }

    /// Releases local callback storage after readers drain, without changing
    /// shared lifecycle state (a replacement may reuse the same mapping).
    pub fn release(&mut self) {
        self.capture_buffer
            .store(ptr::null_mut(), Ordering::Release);
        self.transport.store(ptr::null_mut(), Ordering::Release);
        self.render_buffer.store(ptr::null_mut(), Ordering::Relaxed);
    }
}

fn valid_channels(render: u32, capture: u32) -> bool {
    (render != 0 || capture != 0) && render <= 64 && capture <= 64
}
/// # Safety
/// Context and transport are live. Control has drained all context callbacks;
/// transport mappings remain owned until a later drained unbind.
pub unsafe fn mr_hal_io_bind(context: *mut HalIoContext, transport: *mut c_void) -> bool {
    let Some(context) = (unsafe { context.as_ref() }) else {
        return false;
    };
    let transport = transport.cast::<EndpointTransport>();
    let mut config = EndpointConfig::default();
    if transport.is_null()
        || !unsafe { mr_endpoint_transport_config(transport, &raw mut config) }
        || config.render_channels != context.render_channels
        || config.capture_channels != context.capture_channels
    {
        return false;
    }
    let render = unsafe { mr_endpoint_transport_render_buffer(transport) };
    let capture = unsafe { mr_endpoint_transport_capture_buffer(transport) };
    if (context.render_channels != 0 && render.is_null())
        || (context.capture_channels != 0 && capture.is_null())
    {
        return false;
    }
    if !unsafe { mr_endpoint_transport_begin_clock_transition(transport) } {
        return false;
    }
    context.render_buffer.store(render, Ordering::Relaxed);
    context.capture_buffer.store(capture, Ordering::Relaxed);
    context.transport.store(transport, Ordering::Release);
    unsafe {
        // Binding is preparation, not a ready lifecycle publication. Inherited
        // rate/running fields become visible only after the caller resets PCM
        // epochs and commits through clock_state.
        buffer::mr_shared_audio_buffer_set_producer_activity(
            render,
            context.render_activity.load(Ordering::Acquire),
        );
    }
    true
}

/// # Safety
/// Non-null context is live and all callbacks have drained. Its mapping owner
/// may retire the transport only after this call returns.
pub unsafe fn mr_hal_io_unbind(context: *mut HalIoContext) {
    let Some(context) = (unsafe { context.as_mut() }) else {
        return;
    };
    unsafe {
        mr_endpoint_transport_begin_clock_transition(context.transport.load(Ordering::Acquire));
        buffer::mr_shared_audio_buffer_request_epoch(context.capture());
        mr_endpoint_transport_clock_state(
            context.transport.load(Ordering::Acquire),
            context.clock_epoch,
            context.clock_sample_rate,
            false,
        );
        buffer::mr_shared_audio_buffer_set_producer_activity(
            context.render_buffer.load(Ordering::Acquire),
            0,
        );
    }
    context.release();
}

/// Distinguishes terminal mapping state from an ordinary capture rearm request.
/// # Safety
/// Non-null context and its attached mapping remain live throughout this call.
pub unsafe fn mr_hal_io_requires_replacement(context: *const HalIoContext) -> bool {
    let Some(context) = (unsafe { context.as_ref() }) else {
        return false;
    };
    unsafe {
        mr_endpoint_transport_requires_replacement(context.transport.load(Ordering::Acquire))
            || buffer::mr_shared_audio_buffer_requires_replacement(
                context.render_buffer.load(Ordering::Acquire),
            )
            || buffer::mr_shared_audio_buffer_requires_replacement(context.capture())
    }
}

/// # Safety
/// Context and its attached transport remain live through this call.
pub unsafe fn mr_hal_io_set_render_activity(context: *mut HalIoContext, activity: u32) {
    let Some(context) = (unsafe { context.as_ref() }) else {
        return;
    };
    context.render_activity.store(activity, Ordering::Release);
    unsafe {
        buffer::mr_shared_audio_buffer_set_producer_activity(
            context.render_buffer.load(Ordering::Acquire),
            activity,
        );
    };
}

/// # Safety
/// Control owns context exclusively: clock lifecycle mutations cannot race bind
/// or unbind. PCM callbacks do not read these control-only plain fields.
pub unsafe fn mr_hal_io_clock_state(context: *mut HalIoContext, sample_rate: u32, running: bool) {
    let Some(context) = (unsafe { context.as_mut() }) else {
        return;
    };
    let transport = context.transport.load(Ordering::Acquire);
    if !transport.is_null() && !unsafe { mr_endpoint_transport_begin_clock_transition(transport) } {
        return;
    }
    context.clock_epoch = context.clock_epoch.wrapping_add(1);
    context.clock_sample_rate = sample_rate;
    context.clock_running = running;
    unsafe {
        // No observer can combine the previous clock with a newly reset ring:
        // begin_stream hid the clock before resetting any PCM epoch. In-flight
        // old writers retain their expected epoch and fail after this rearm;
        // writer claims prevent consuming rearm until the preceding writer drains.
        // HAL owns both writers, so transitional render PCM must not survive a rate commit
        // either. Otherwise its old watermark rejects the first block in the new timeline.
        for ring in [
            context.render_buffer.load(Ordering::Relaxed),
            context.capture(),
        ] {
            buffer::mr_shared_audio_buffer_request_epoch(ring);
            buffer::mr_shared_audio_buffer_try_begin_epoch(ring);
        }
        mr_endpoint_transport_clock_state(transport, context.clock_epoch, sample_rate, running);
    };
}

/// Whether `mr_hal_io_begin_stream` would rearm every endpoint ring, checked without
/// changing any shared state, so a caller can refuse before mutating anything else.
/// # Safety
/// Non-null context and its attached mapping remain live; control has drained its writers.
pub unsafe fn can_begin_stream(context: *const HalIoContext) -> bool {
    let Some(context) = (unsafe { context.as_ref() }) else {
        return false;
    };
    let transport = context.transport.load(Ordering::Acquire);
    let capture = context.capture();
    let render = context.render_buffer.load(Ordering::Relaxed);
    !transport.is_null()
        && (context.capture_channels == 0 || !capture.is_null())
        && (context.render_channels == 0 || !render.is_null())
        && !unsafe { mr_endpoint_transport_requires_replacement(transport) }
        && [render, capture]
            .into_iter()
            // SAFETY: each non-null ring belongs to the live attached mapping.
            .all(|ring| ring.is_null() || unsafe { buffer::can_begin_epoch(ring) })
}

/// # Safety
/// Control has serialized lifecycle transitions and drained this HAL writer.
/// Remote readers need not be drained: no shared storage clears.
pub unsafe fn mr_hal_io_begin_stream(context: *mut HalIoContext) -> bool {
    let Some(context) = (unsafe { context.as_ref() }) else {
        return false;
    };
    let transport = context.transport.load(Ordering::Acquire);
    let capture = context.capture();
    let render = context.render_buffer.load(Ordering::Relaxed);
    // A client start is lifecycle evidence even if the mapping has not arrived.
    // Retain it in the context so a later bind publishes AwaitingRender instead
    // of falsely reporting inactivity throughout the client's running lifetime.
    context.render_activity.store(2, Ordering::Release);
    unsafe { buffer::mr_shared_audio_buffer_set_producer_activity(render, 2) };
    if transport.is_null()
        || (context.capture_channels != 0 && capture.is_null())
        || (context.render_channels != 0 && render.is_null())
    {
        return false;
    }
    if !unsafe { mr_endpoint_transport_begin_clock_transition(transport) } {
        return false;
    }
    unsafe {
        for ring in [render, capture] {
            if ring.is_null() {
                continue;
            }
            if buffer::mr_shared_audio_buffer_try_begin_epoch(ring) == 0 {
                buffer::mr_shared_audio_buffer_retire_busy_producer_after_drain(ring);
                return false;
            }
        }
        true
    }
}

/// # Safety
/// Non-null output points to one writable u64.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_hal_io_sample_frame(time: f64, frame: *mut u64) -> bool {
    if frame.is_null()
        || !time.is_finite()
        || !(0.0..=MAX_EXACT_FRAME).contains(&time)
        || time.fract() != 0.0
    {
        return false;
    }
    // Decode the integral IEEE-754 value instead of an unchecked float cast.
    let bits = time.to_bits();
    let converted = if time < 1.0 {
        0
    } else {
        let exponent = (bits >> 52) & 0x7ff;
        let significand = (bits & ((1_u64 << 52) - 1)) | (1_u64 << 52);
        significand >> (1075 - exponent)
    };
    unsafe {
        frame.write(converted);
    }
    true
}

#[cfg(test)]
#[path = "unit_tests/hal_io.rs"]
mod tests;
