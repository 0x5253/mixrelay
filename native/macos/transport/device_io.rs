//! One retained device owner with disjoint control, callback and graph state.
//! The engine owns the stable allocation and releases it only after unregister
//! acknowledges callback drain and listener removal; failed destruction retains the owner.

use std::{
    alloc::{Layout, alloc},
    cell::UnsafeCell,
    ffi::{CStr, c_void},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
};

use mixrelay_pcm_ring::PcmRing;

// Both native and graph queues retain the same backlog across live rate changes.
// Their latency targets still follow observed callback bursts, not this capacity.
const FIFO_FRAMES: u32 = mixrelay_audio_io::retention::RING_FRAMES;

use crate::{
    callback_clock::CallbackClock,
    device_buffers::{BufferSelection, NativeFormats},
    device_io::lifecycle::DeviceIoLifecycle,
};

mod callback;
mod control;
mod diagnostics;
mod execution;
mod ffi;
mod latency;
mod lifecycle;
pub use latency::{LatencySnapshot, latency_snapshot, record_direct_latency};
mod listeners;
mod native;
mod queues;
mod stream_format;
mod stream_usage;
mod timestamps;
pub use timestamps::{
    bridge_read_timed, bridge_write_timed, capture_burst, capture_window, frame_nanoseconds,
    graph_read_at, input_time, set_capture_window,
};
#[cfg(any(test, feature = "test-observation"))]
pub mod test_observation;
#[cfg(test)]
mod tests;

pub use callback::mr_device_io_process;

pub use diagnostics::{
    CLOCK_BREAK_CAPACITY, ClockBreak, ClockBreakRecord, mr_device_io_next_clock_break,
};
pub use ffi::{
    mr_device_buffer_size, mr_device_buffer_write, mr_device_io_alive, mr_device_io_bridge_discard,
    mr_device_io_bridge_read, mr_device_io_bridge_write, mr_device_io_buffer_info,
    mr_device_io_callback_host, mr_device_io_clock_device, mr_device_io_clock_discontinuities,
    mr_device_io_connect, mr_device_io_connect_status, mr_device_io_create,
    mr_device_io_create_group, mr_device_io_destroy, mr_device_io_direct_binding,
    mr_device_io_disconnect, mr_device_io_graph_frames, mr_device_io_graph_read,
    mr_device_io_graph_write, mr_device_io_invalid, mr_device_io_native_frames,
    mr_device_io_native_rate, mr_device_io_output_epoch, mr_device_io_overflows,
    mr_device_io_pcm_hook, mr_device_io_quantum, mr_device_io_queued, mr_device_io_render_hook,
    mr_device_io_starvations, mr_device_io_ticks,
};
pub use queues::GraphRead;

const MAX_FRAMES: u32 = 4096;

/// Borrowed engine render hook, installed only before registration. The hook must
/// not retain native buffers or outlive its enclosing engine render-slot owner.
/// Validity-preserving native timing for both callback directions.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct IoTimestamp {
    /// Sample position in this device's clock only.
    pub sample_time: f64,
    /// System host ticks; convert with the prepared host timebase.
    pub host_time: u64,
    /// Actual versus nominal device rate.
    pub rate_scalar: f64,
    /// Native validity flags: sample=1, host=2, rate scalar=4.
    pub flags: u32,
}

/// Borrowed timing accompanies native PCM, never retained past the callback.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct IoTiming {
    pub now: IoTimestamp,
    pub input: IoTimestamp,
    pub output: IoTimestamp,
}

pub type RenderHook =
    unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void, u32, *const IoTiming) -> bool;

/// Private callback stages. Input admission precedes capture; conversion follows
/// it in the same callback. Output conversion runs only during preparation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PcmPhase {
    Prepare = 0,
    Captured = 1,
}

impl PcmPhase {
    #[must_use]
    pub const fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Prepare),
            1 => Some(Self::Captured),
            _ => None,
        }
    }
}

/// Serialized clock-boundary processor. Prepare returns admission; Captured
/// reports whether the admitted input was processed. Only designated SRC queue
/// cursors may be accessed. Raw phase values are validated at the Rust boundary.
pub type PcmHook = unsafe extern "C" fn(*mut c_void, u32) -> bool;

struct Selection {
    uid: Vec<u8>,
    input: bool,
    channels: u32,
    // The enclosing engine Native retains the aggregate Arc beyond unregister.
    group: *mut c_void,
}

#[derive(Default)]
struct Connection {
    binding: crate::aggregate_group::NativeBinding,
    rate: u32,
    formats: NativeFormats,
    // Shared only with this registration's listener block and callback.
    watch_invalid: Arc<AtomicBool>,
}

#[derive(Default)]
struct Control {
    lifecycle: DeviceIoLifecycle,
    connection: Connection,
    listeners: listeners::Listeners,
}

struct Callback {
    selection: BufferSelection,
    formats: NativeFormats,
    watch_invalid: Arc<AtomicBool>,
    scratch: Vec<f32>,
    prime: bool,
    epoch: u64,
    hook: Option<RenderHook>,
    context: *mut c_void,
    pcm_hook: Option<PcmHook>,
    pcm_context: *mut c_void,
    previous_timestamp: Option<(f64, u64, u32)>,
    /// Host ticks per frame between this device's consecutive callback timestamps.
    frame_ticks: f64,
    timebase: mixrelay_realtime::Timebase,
    acquisition_nanos: u64,
    frame_nanos: f64,
}

#[derive(Default)]
struct Progress {
    output_only: AtomicBool,
    ticks: AtomicU64,
    native_frames: AtomicU64,
    graph_frames: AtomicU64,
    output_epoch: AtomicU64,
    native_quantum: AtomicU32,
    graph_quantum: AtomicU32,
    invalid: AtomicBool,
    /// The latest callback's mean-frame host ticks; its own queue roles pass frames then.
    callback_host: AtomicU64,
    frame_nanos: AtomicU64,
    capture_window: AtomicU32,
}

/// Opaque device context. No blanket `Send`/`Sync`: the enclosing engine owner
/// separately grants control, callback and graph access to their designated cells.
pub struct DeviceIo {
    selection: Selection,
    credit_signal: Option<Arc<dyn Fn() + Send + Sync>>,
    timebase: mixrelay_realtime::Timebase,
    control: UnsafeCell<Control>,
    callback: UnsafeCell<Callback>,
    native_ring: PcmRing,
    graph_ring: PcmRing,
    native_times: crate::frame_times::FrameTimes,
    graph_times: crate::frame_times::FrameTimes,
    graph_epoch: UnsafeCell<Option<u64>>,
    capture_cadence: UnsafeCell<timestamps::CaptureCadence>,
    clock: CallbackClock,
    clock_evidence: diagnostics::ClockEvidence,
    execution: execution::Execution,
    progress: Progress,
    measurements: latency::Measurements,
}

impl DeviceIo {
    /// Installs this owner's realtime-safe credit notification before registration.
    /// The enclosing owner retains exclusive access until registration publishes callbacks.
    pub fn set_credit_signal(&mut self, signal: Arc<dyn Fn() + Send + Sync>) {
        self.credit_signal = Some(signal);
    }

    /// Declares that the next registration will never use native input, including
    /// through a direct-render hook. Configure only while disconnected.
    pub fn set_output_only(&self, output_only: bool) {
        self.progress
            .output_only
            .store(output_only, Ordering::Release);
    }

    #[cfg(test)]
    fn for_test(uid: &CStr, input: bool, channels: u32, rate: u32) -> Option<Self> {
        Self::new(
            uid,
            input,
            channels,
            rate,
            mixrelay_realtime::Timebase::from_ratio(
                std::num::NonZeroU32::MIN,
                std::num::NonZeroU32::MIN,
            ),
        )
    }

    fn new(
        uid: &CStr,
        input: bool,
        channels: u32,
        rate: u32,
        timebase: mixrelay_realtime::Timebase,
    ) -> Option<Self> {
        if !(1..=64).contains(&channels) || rate == 0 {
            return None;
        }
        let mut name = Vec::new();
        name.try_reserve_exact(uid.to_bytes_with_nul().len()).ok()?;
        name.extend_from_slice(uid.to_bytes_with_nul());
        let mut scratch = Vec::new();
        scratch
            .try_reserve_exact(MAX_FRAMES as usize * channels as usize)
            .ok()?;
        scratch.resize(MAX_FRAMES as usize * channels as usize, 0.0);
        Some(Self {
            credit_signal: None,
            timebase,
            selection: Selection {
                uid: name,
                input,
                channels,
                group: ptr::null_mut(),
            },
            control: UnsafeCell::new(Control {
                connection: Connection {
                    rate,
                    ..Connection::default()
                },
                ..Control::default()
            }),
            callback: UnsafeCell::new(Callback {
                selection: BufferSelection {
                    channels,
                    offset: 0,
                    total: 0,
                    limit: 64,
                },
                formats: NativeFormats::default(),
                watch_invalid: Arc::default(),
                scratch,
                prime: false,
                epoch: 0,
                hook: None,
                context: ptr::null_mut(),
                pcm_hook: None,
                pcm_context: ptr::null_mut(),
                previous_timestamp: None,
                frame_ticks: 0.0,
                timebase,
                acquisition_nanos: 0,
                frame_nanos: 1e9 / f64::from(rate),
            }),
            native_ring: PcmRing::new(channels, FIFO_FRAMES)?,
            graph_ring: PcmRing::new(channels, FIFO_FRAMES)?,
            native_times: crate::frame_times::FrameTimes::new(FIFO_FRAMES as usize)?,
            graph_times: crate::frame_times::FrameTimes::new(FIFO_FRAMES as usize)?,
            graph_epoch: UnsafeCell::new(None),
            capture_cadence: UnsafeCell::new(timestamps::CaptureCadence::default()),
            clock: CallbackClock::default(),
            clock_evidence: diagnostics::ClockEvidence::default(),
            execution: execution::Execution::default(),
            progress: Progress::default(),
            measurements: latency::Measurements::default(),
        })
    }

    fn publish(self) -> *mut Self {
        // SAFETY: one correctly aligned allocation becomes the stable IOProc context.
        // A failed allocation drops the unpublished owner and all prepared resources.
        let allocation = unsafe { alloc(Layout::new::<Self>()) }
            .cast::<c_void>()
            .cast::<Self>();
        if !allocation.is_null() {
            // SAFETY: allocation is exclusively owned and large enough for DeviceIo.
            unsafe { allocation.write(self) };
        }
        allocation
    }

    fn ring(&self, graph: bool) -> &PcmRing {
        if graph {
            &self.graph_ring
        } else {
            &self.native_ring
        }
    }

    /// Each role records the ring cursor it alone advances, so both sides count the same
    /// frames, including any it discarded. `host` is that role's own host time.
    fn produced(&self, graph: bool, frames: u32, host: u64) {
        self.passage(graph).producer.advance(
            self.ring(graph).stats().written,
            u64::from(frames),
            host,
        );
    }
    fn consumed(&self, graph: bool, frames: u32, host: u64) {
        self.passage(graph).consumer.advance(
            self.ring(graph).stats().read,
            u64::from(frames),
            host,
        );
    }
    fn passage(&self, graph: bool) -> &mixrelay_audio_io::residence::Passage {
        if graph {
            &self.measurements.graph_queue
        } else {
            &self.measurements.native_queue
        }
    }
}
