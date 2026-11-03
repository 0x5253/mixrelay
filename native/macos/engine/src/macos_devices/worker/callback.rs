//! Prepared conversion belongs exclusively to the native callback while published.
use super::WorkerContext;
use super::metrics::ResetCause;
#[cfg(test)]
mod tests;
use crate::macos_devices::{bridge::DeviceBridge, ffi, ffi::PcmPhase};
use std::{
    cell::UnsafeCell,
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering},
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(in crate::macos_devices) struct PcmSlot {
    state: AtomicPtr<c_void>,
    readers: AtomicU32,
    rendering: AtomicBool,
    fault: AtomicU32,
}

impl PcmSlot {
    pub(super) fn failure(&self) -> Option<&'static str> {
        match self.fault.load(Ordering::Acquire) {
            0 => None,
            1 => Some("invalid callback bridge configuration"),
            2 => Some("invalid or nonfinite callback PCM"),
            3 => Some("callback bridge FIFO overflow"),
            4 => Some("invalid callback clock feedback"),
            5 => Some("callback sample-rate converter failure"),
            _ => Some("unknown callback bridge failure"),
        }
    }
    fn drain(&self) {
        self.state.store(ptr::null_mut(), Ordering::SeqCst);
        while self.readers.load(Ordering::SeqCst) != 0 {
            thread::park_timeout(Duration::from_micros(100));
        }
    }
}

struct State<'a> {
    context: &'a WorkerContext,
    bridge: DeviceBridge,
    generation: u64,
    output_started: bool,
    capture_prepared: bool,
    preparation_work: Duration,
}

impl State<'_> {
    fn configured(&self) -> bool {
        self.bridge.graph_rate == self.context.graph_rate.load(Ordering::Acquire)
            && self.bridge.clock_aligned == self.context.clock_aligned.load(Ordering::Acquire)
    }

    fn prepare(&mut self) -> Result<bool, mixrelay_audio_io::clock_bridge::BridgeError> {
        let context = self.context;
        self.capture_prepared = false;
        if !self.configured() {
            context.ready_rate.store(0, Ordering::Release);
            return Ok(false);
        }
        let generation = context.native.render.generation.load(Ordering::Acquire);
        let pointer = context.native.pointer.as_ptr();
        // SAFETY: the retained owner exposes only atomic counters here.
        let (overflows, clock_gaps) = unsafe {
            (
                ffi::mr_device_io_overflows(pointer),
                ffi::mr_device_io_clock_discontinuities(pointer),
            )
        };
        if generation != self.generation
            || overflows != self.bridge.transport_overflows
            || clock_gaps != self.bridge.clock_discontinuities
        {
            let cause = if generation != self.generation {
                ResetCause::DirectExit
            } else if clock_gaps != self.bridge.clock_discontinuities {
                ResetCause::ClockGap
            } else {
                ResetCause::Overflow
            };
            self.bridge.restart(context);
            self.generation = generation;
            context.native.pcm_metrics.reset(cause);
            self.output_started = false;
            if !context.input {
                return Ok(false);
            }
        }
        let reset_activity = !context.activity.ready();
        if !self.bridge.synchronize_activity(context) {
            self.output_started = false;
            return Ok(false);
        }
        if reset_activity && !context.input {
            self.output_started = false;
            return Ok(false);
        }
        if context.input {
            self.capture_prepared = true;
            return Ok(true);
        }
        self.transfer()
    }

    fn captured(&mut self) -> Result<bool, mixrelay_audio_io::clock_bridge::BridgeError> {
        if !self.context.input {
            return Err(mixrelay_audio_io::clock_bridge::BridgeError::InvalidConfig);
        }
        if !std::mem::take(&mut self.capture_prepared) {
            // Control may have replaced this converter between native stages.
            // Its new owner never admitted the previous format's capture packet.
            self.invalidate();
            return Ok(false);
        }
        self.transfer()
    }

    fn invalidate(&mut self) {
        self.bridge.restart(self.context);
        self.context
            .epoch
            .store(self.bridge.epoch, Ordering::Release);
        self.context.ready_rate.store(0, Ordering::Release);
        self.capture_prepared = false;
        self.output_started = false;
    }

    fn transfer(&mut self) -> Result<bool, mixrelay_audio_io::clock_bridge::BridgeError> {
        if !self.configured() {
            self.invalidate();
            return Ok(false);
        }
        let context = self.context;
        let pointer = context.native.pointer.as_ptr();
        self.bridge.observe(context, true)?;
        self.bridge.pump(context)?;
        let (pending, _) = self.bridge.feedback_occupancy(context);
        context.native.pcm_metrics.buffering(
            pending,
            if context.input {
                self.bridge.native_rate
            } else {
                self.bridge.graph_rate
            },
            self.bridge.bridge.correction_ppm(),
        );
        context.epoch.store(self.bridge.epoch, Ordering::Release);
        context
            .ready_rate
            .store(self.bridge.graph_rate, Ordering::Release);
        if !context.input && !self.output_started {
            // Prime with real converted PCM. Native output remains silent until
            // an entire prepared transport window can survive converter batching.
            let queued = unsafe { ffi::mr_device_io_queued(pointer, false) } as usize;
            self.output_started = queued >= self.bridge.queue_target(context, false);
        }
        Ok(context.input || self.output_started)
    }
}

/// The connection owns this guard and drops it before disconnecting native I/O.
/// Control changes require unpublication and reader drain; no mutable reference
/// to converter storage exists while any native callback can access it.
pub(in crate::macos_devices) struct CallbackBridge<'a> {
    slot: &'a PcmSlot,
    state: Box<UnsafeCell<State<'a>>>,
}

impl<'a> CallbackBridge<'a> {
    pub(in crate::macos_devices) fn new(
        context: &'a WorkerContext,
        rate: u32,
    ) -> Result<Self, String> {
        let bridge = DeviceBridge::new(context, rate)?;
        let result = Self {
            slot: &context.native.pcm,
            state: Box::new(UnsafeCell::new(State {
                context,
                bridge,
                generation: context.native.render.generation.load(Ordering::Acquire),
                output_started: false,
                capture_prepared: false,
                preparation_work: Duration::ZERO,
            })),
        };
        result.slot.fault.store(0, Ordering::Release);
        result.publish();
        Ok(result)
    }

    fn publish(&self) {
        self.slot
            .state
            .store(self.state.get().cast(), Ordering::SeqCst);
    }

    pub(super) fn replace(&mut self, mut bridge: DeviceBridge) {
        self.slot.drain();
        let state = self.state.get_mut();
        bridge.activate(state.context);
        state.context.native.pcm_metrics.reset(ResetCause::Format);
        state.bridge = bridge;
        state.output_started = false;
        state.capture_prepared = false;
        state.preparation_work = Duration::ZERO;
        self.publish();
    }
}

impl Drop for CallbackBridge<'_> {
    fn drop(&mut self) {
        self.slot.drain();
    }
}

/// Called by the retained native registration, never by the control owner.
pub(in crate::macos_devices) unsafe extern "C" fn dispatch(
    context: *mut c_void,
    phase: u32,
) -> bool {
    let Some(phase) = PcmPhase::from_raw(phase) else {
        return false;
    };
    // SAFETY: Native pins this slot until after native unregister acknowledges drain.
    let slot = unsafe { &*context.cast::<PcmSlot>() };
    slot.readers.fetch_add(1, Ordering::SeqCst);
    let state = slot.state.load(Ordering::SeqCst);
    let mut admitted = false;
    if !state.is_null()
        && slot.fault.load(Ordering::Acquire) == 0
        && slot
            .rendering
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    {
        // SAFETY: reader announcement protects the guard's pinned State and context.
        // The rendering token grants the only mutable borrow; control drains first.
        let state = unsafe { &mut *state.cast::<State<'_>>() };
        let started = Instant::now();
        if phase == PcmPhase::Prepare {
            state.preparation_work = Duration::ZERO;
        }
        let result = match phase {
            PcmPhase::Prepare => state.prepare(),
            PcmPhase::Captured => state.captured(),
        };
        match result {
            Ok(active) => admitted = active,
            Err(error) => {
                use mixrelay_audio_io::clock_bridge::BridgeError;
                let code = match error {
                    BridgeError::InvalidConfig | BridgeError::Allocation { .. } => 1,
                    BridgeError::InvalidBuffer => 2,
                    BridgeError::Overflow => 3,
                    BridgeError::InvalidFeedback => 4,
                    BridgeError::Resampler => 5,
                };
                // A failed post-capture conversion must withdraw old graph PCM
                // and discard remaining native input before any future admission.
                state.invalidate();
                slot.fault.store(code, Ordering::Release);
            }
        }
        // This field access stays inside the exclusive rendering borrow.
        if !admitted {
            state
                .context
                .native
                .latency
                .faults
                .fetch_add(1, Ordering::Release);
        }
        if phase == PcmPhase::Prepare && state.context.input && admitted {
            state.preparation_work = started.elapsed();
        } else {
            // One work observation covers both input phases; the controller,
            // residences and conversion advance only in Captured.
            let elapsed = std::mem::take(&mut state.preparation_work) + started.elapsed();
            // SAFETY: atomic geometry observation of the retained native owner.
            let quantum =
                unsafe { ffi::mr_device_io_quantum(state.context.native.pointer.as_ptr(), false) };
            state
                .context
                .native
                .pcm_metrics
                .work(elapsed, quantum, state.bridge.native_rate);
        }
        slot.rendering.store(false, Ordering::Release);
    }
    slot.readers.fetch_sub(1, Ordering::SeqCst);
    admitted
}
