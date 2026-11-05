//! The IOProc-side renderer: runs the graph synchronously inside each native
//! callback, splitting the native quantum into graph blocks. Nothing here
//! allocates, locks or waits.
use super::{
    Binding, DirectPlan, buffers::Buffers, callback_io::CallbackIo, supervision::RenderSignals,
};
use crate::macos::AudioContext;
use mixrelay_audio_io::{
    process::{ClockDomain, ProcessCycle},
    session::ClockAuthority,
};
use mixrelay_engine_core::session::AdapterRuntimeStatus;
use mixrelay_macos_transport::device_io::{IoTimestamp, IoTiming};
use mixrelay_render::{BlockProcessor, RenderPlan};
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::Ordering,
};

/// The largest native callback quantum the renderer accepts.
const MAX_CALLBACK_FRAMES: u32 = 4096;

pub(super) struct Renderer<'a> {
    pub(super) timebase: mixrelay_realtime::Timebase,
    pub(super) processor: &'a mut Box<BlockProcessor>,
    pub(super) activation: Option<GraphActivation<'a>>,
    pub(super) bindings: &'a [Binding],
    pub(super) clock_id: u32,
    pub(super) epoch: u64,
    pub(super) native_clock: Option<(&'a DirectPlan, u64)>,
    pub(super) status: &'a AdapterRuntimeStatus,
    /// The first frame expected next, in the timeline last used; `None` restarts it.
    pub(super) position: Option<u64>,
    pub(super) signals: &'a RenderSignals,
}

/// The installed native geometry is immutable throughout a direct lifetime.
/// A compatible DSP replacement can use it without withdrawing the callback or
/// rebuilding independent-clock converters. Other plans remain queued for rebind.
pub(super) struct GraphActivation<'a> {
    pub(super) installed: &'a RenderPlan,
    pub(super) context: AudioContext<'a>,
}

impl GraphActivation<'_> {
    /// Activates a pending plan that fits the installed geometry. Any other plan stays
    /// pending and raises `signals.rebind`.
    pub(super) fn activate(
        &self,
        processor: &mut Box<BlockProcessor>,
        signals: &RenderSignals,
    ) -> bool {
        if !self.context.exchange.has_pending() && !self.context.exchange.has_pending_latency() {
            return true;
        }
        self.context
            .exchange
            .activate_pending_if_configured(
                processor,
                |next| {
                    let compatible = !self.context.devices.has_pending()
                        && same_external_geometry(self.installed, next);
                    if !compatible {
                        signals.rebind.store(true, Ordering::Release);
                    }
                    compatible
                },
                |_| Ok::<(), String>(()),
            )
            .is_ok()
    }
}

fn same_external_geometry(installed: &RenderPlan, next: &RenderPlan) -> bool {
    installed.format() == next.format()
        && installed.source_bindings() == next.source_bindings()
        && installed.sink_bindings() == next.sink_bindings()
}

/// The native `IOProc` entry. A panic or rejected cycle marks the renderer failed.
pub(super) unsafe extern "C" fn render(
    context: *mut c_void,
    input: *const c_void,
    output: *mut c_void,
    frames: u32,
    timing: *const IoTiming,
) {
    // SAFETY: the descriptor is published to exactly one serialized IOProc;
    // control never accesses this renderer until removal + reader drain.
    let renderer = unsafe { &mut *context.cast::<Renderer<'_>>() };
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        renderer.render(input, output, frames, timing.as_ref())
    }));
    if !matches!(result, Ok(true)) {
        renderer.signals.failed.store(true, Ordering::Release);
    }
}

impl<'a> Renderer<'a> {
    /// A renderer clocked by `plan`'s device in a fresh epoch, without graph activation.
    pub(super) fn for_plan(
        plan: &'a DirectPlan,
        processor: &'a mut Box<BlockProcessor>,
        status: &'a AdapterRuntimeStatus,
        signals: &'a RenderSignals,
    ) -> Self {
        Self {
            timebase: mixrelay_realtime::Timebase::current(),
            processor,
            activation: None,
            bindings: &plan.bindings,
            clock_id: plan.clock_id,
            epoch: crate::macos_devices::direct::next_epoch(),
            native_clock: Some((plan, plan.discontinuities())),
            status,
            position: None,
            signals,
        }
    }
}

impl Renderer<'_> {
    /// Restarts the frame timeline in a new epoch after a hardware discontinuity.
    fn observe_hardware(&mut self) {
        if let Some((plan, observed)) = &mut self.native_clock {
            let current = plan.discontinuities();
            if current != *observed {
                self.status
                    .record_process(mixrelay_audio_io::process::ProcessReport {
                        discontinuities: current.wrapping_sub(*observed),
                        ..Default::default()
                    });
                self.epoch = crate::macos_devices::direct::next_epoch();
                self.position = None;
                *observed = current;
            }
        }
    }

    /// Returns false when the renderer must stop; an unusable buffer layout is
    /// reported through `invalid` while the callback itself completes.
    pub(super) unsafe fn render(
        &mut self,
        input: *const c_void,
        output: *mut c_void,
        frames: u32,
        timing: Option<&IoTiming>,
    ) -> bool {
        if self.signals.failed.load(Ordering::Acquire)
            || frames == 0
            || frames > MAX_CALLBACK_FRAMES
        {
            return false;
        }
        if self
            .activation
            .as_ref()
            .is_some_and(|activation| !activation.activate(self.processor, self.signals))
        {
            return false;
        }
        self.observe_hardware();
        // SAFETY: the serialized native callback retains both ABLs and their PCM;
        // DSP scratch belongs to the independent BlockProcessor allocation. The
        // closure cannot return a view or retain access past this synchronous call.
        let result = unsafe {
            Buffers::with_native(input, output, frames, |buffers| {
                self.render_buffers(buffers, frames, timing)
            })
        };
        result.unwrap_or_else(|| {
            self.signals.invalid.store(true, Ordering::Release);
            true
        })
    }

    fn render_buffers(
        &mut self,
        buffers: Buffers<'_>,
        frames: u32,
        timing: Option<&IoTiming>,
    ) -> bool {
        if !self.bindings.iter().all(|binding| buffers.covers(binding)) {
            self.signals.invalid.store(true, Ordering::Release);
            return true;
        }
        let mut io = CallbackIo {
            buffers,
            bindings: self.bindings,
            offset: 0,
            bridged: self
                .native_clock
                .as_ref()
                .map(|(plan, _)| (&plan.bridged, plan.callback_host())),
        };
        let Some(first_frame) = self.process_quanta(&mut io, frames, timing) else {
            return false;
        };
        self.position = Some(first_frame + u64::from(frames));
        self.status
            .observe_clock(ClockAuthority::Device, self.clock_synchronized());
        self.signals.progress.fetch_add(1, Ordering::Release);
        true
    }

    /// Processes every graph block of one native callback in order and returns
    /// the callback's first frame, or `None` when the graph rejected a cycle.
    fn process_quanta(
        &mut self,
        io: &mut CallbackIo<'_>,
        frames: u32,
        timing: Option<&IoTiming>,
    ) -> Option<u64> {
        let time = timing.map(|timing| {
            self.native_clock
                .as_ref()
                .map_or(timing.output, |(plan, _)| plan.cycle_time(timing))
        });
        let (first_frame, mut discontinuity) =
            callback_start(time.and_then(native_sample), self.position);
        let format = self.processor.plan().format();
        for (offset, count) in quanta(frames, format.block_frames) {
            io.offset = offset as usize;
            let cycle = ProcessCycle {
                clock: ClockDomain {
                    authority: ClockAuthority::Device,
                    id: u64::from(self.clock_id),
                    epoch: self.epoch,
                },
                sample_rate: format.sample_rate,
                frames: count,
                first_frame: first_frame + u64::from(offset),
                monotonic_ns: time.filter(|time| time.flags & 2 != 0).map(|time| {
                    self.timebase.nanoseconds(time.host_time)
                        + u64::from(offset) * 1_000_000_000 / u64::from(format.sample_rate)
                }),
                discontinuity: std::mem::take(&mut discontinuity),
            };
            let Ok(Some(report)) = self.processor.process_external_cycle(cycle, io) else {
                return None;
            };
            if let Some((plan, _)) = &self.native_clock {
                plan.record_latency(self.processor, report, timing.copied(), cycle);
            }
            crate::macos::runtime_latency::record_sources(
                self.processor,
                crate::endpoint_service::source_registry(),
                true,
                cycle.clock.epoch,
            );
            self.status.record_process(report);
        }
        Some(first_frame)
    }

    /// Whether every bridged endpoint follows this device's clock.
    fn clock_synchronized(&self) -> bool {
        self.native_clock.as_ref().is_none_or(|(plan, _)| {
            [
                (true, self.processor.plan().source_bindings()),
                (false, self.processor.plan().sink_bindings()),
            ]
            .into_iter()
            .all(|(input, bindings)| {
                bindings.iter().all(|binding| {
                    plan.bridged
                        .clock_synchronized(binding.node_id().as_str(), input)
                        .unwrap_or(true)
                })
            })
        })
    }
}

/// The callback's first frame and whether it breaks continuity. Native sample
/// time wins when the device reports it, otherwise the previous position
/// continues. Any jump (a device skip, or switching between the two) is a
/// discontinuity rather than a failure.
fn callback_start(native: Option<u64>, expected: Option<u64>) -> (u64, bool) {
    let first = native.or(expected).unwrap_or(0);
    (first, expected != Some(first))
}

/// Splits a native quantum into graph blocks as `(offset, frames)`. A larger
/// native quantum is split, never truncated or queued for later.
fn quanta(frames: u32, block_frames: u32) -> impl Iterator<Item = (u32, u32)> {
    let mut offset = 0;
    std::iter::from_fn(move || {
        (offset < frames).then(|| {
            let count = (frames - offset).min(block_frames);
            let block = (offset, count);
            offset += count;
            block
        })
    })
}

/// A device sample time usable as an exact frame index.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn native_sample(time: IoTimestamp) -> Option<u64> {
    (time.flags & 1 != 0
        && time.sample_time.is_finite()
        && time.sample_time >= 0.0
        && time.sample_time < 9_007_199_254_740_992.0)
        .then_some(time.sample_time as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_time_starts_the_callback_and_any_jump_is_a_discontinuity() {
        assert_eq!(callback_start(None, None), (0, true));
        assert_eq!(callback_start(None, Some(32)), (32, false));
        assert_eq!(callback_start(Some(32), Some(32)), (32, false));
        assert_eq!(callback_start(Some(96_000), Some(32)), (96_000, true));
        assert_eq!(callback_start(Some(96_000), None), (96_000, true));
    }

    #[test]
    fn quanta_cover_the_native_callback_without_truncation() {
        assert_eq!(quanta(32, 32).collect::<Vec<_>>(), [(0, 32)]);
        assert_eq!(quanta(49, 32).collect::<Vec<_>>(), [(0, 32), (32, 17)]);
        assert_eq!(quanta(16, 32).collect::<Vec<_>>(), [(0, 16)]);
        assert_eq!(quanta(0, 32).count(), 0);
    }

    #[test]
    fn only_valid_finite_sample_times_become_frame_indices() {
        let time = |sample_time, flags| IoTimestamp {
            sample_time,
            host_time: 0,
            rate_scalar: 1.0,
            flags,
        };
        assert_eq!(native_sample(time(48_000.0, 1)), Some(48_000));
        assert_eq!(native_sample(time(48_000.0, 2)), None);
        for invalid in [f64::NAN, f64::INFINITY, -1.0, 9_007_199_254_740_992.0] {
            assert_eq!(native_sample(time(invalid, 1)), None);
        }
    }
}
