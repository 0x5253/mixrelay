//! Callback-driven hardware rendering. All maps/processor storage are prepared
//! before publication. The supervisor only handles reconfiguration and lifetime;
//! it never schedules PCM or acknowledges pause while a render remains in flight.
use super::AudioContext;
#[cfg(test)]
mod continuity_tests;
mod ownership;
use crate::macos_devices::direct::{Binding, Descriptor, DirectPlan};
use mixrelay_audio_engine::{BlockProcessor, ExternalBinding, ExternalIo, RenderPlan};
use mixrelay_audio_io::{
    process::{ClockDomain, InputActivity, ProcessCycle, TransferOutcome},
    session::ClockAuthority,
};
use ownership::PreparedRenderer;
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

mod buffers;
use buffers::Buffers;

#[cfg(test)]
use mixrelay_macos_transport::audio_buffer::AudioBuffer;

#[cfg(test)]
#[repr(C)]
struct AudioBufferList {
    count: u32,
    first: AudioBuffer,
}

struct CallbackIo<'a> {
    buffers: Buffers<'a>,
    bindings: &'a [Binding],
    offset: usize,
    /// Bridged devices and the host time labelling their transfers in this callback.
    bridged: Option<(&'a crate::macos_devices::DeviceSet, u64)>,
}
impl ExternalIo for CallbackIo<'_> {
    fn source_epoch(&self, binding: &ExternalBinding) -> Option<u64> {
        // The native binding owns its clock and continuity. A registry epoch
        // belongs only to a source actually read from the shared transport.
        let native = self
            .bindings
            .iter()
            .any(|map| map.input && map.id == binding.node_id().as_str())
            || self.bridged.is_some_and(|(devices, _)| {
                devices.binding_present(binding.node_id().as_str(), true)
            });
        if native {
            return None;
        }
        super::application_epoch(binding, super::endpoint_service::source_registry())
    }

    fn begin_cycle(
        &mut self,
        _cycle: &ProcessCycle,
        controls: mixrelay_audio_engine::ExternalControlState<'_>,
    ) -> mixrelay_audio_engine::CycleAdmission {
        if let Some((devices, _)) = self.bridged {
            for (binding, enabled) in controls.sources() {
                devices.set_binding_active(binding.node_id().as_str(), true, enabled);
            }
            for (binding, enabled) in controls.sinks() {
                devices.set_binding_active(binding.node_id().as_str(), false, enabled);
            }
        }
        mixrelay_audio_engine::CycleAdmission::Process
    }
    fn read_source(
        &mut self,
        binding: &ExternalBinding,
        destination: &mut [f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        if let Some(result) = self
            .bridged
            .and_then(|(devices, host)| devices.read(binding.node_id().as_str(), destination, host))
        {
            return result;
        }
        let Some(map) = self
            .bindings
            .iter()
            .find(|map| map.input && map.id == binding.node_id().as_str())
        else {
            return super::read_application(
                binding,
                destination,
                cycle,
                super::endpoint_service::source_registry(),
                self.bridged.map(|(_, host)| host),
            );
        };
        if map.offset.is_none() {
            return TransferOutcome {
                activity: InputActivity::Inactive,
                ..TransferOutcome::complete(cycle.frames)
            };
        }
        self.buffers.transfer(map, self.offset, destination);
        TransferOutcome::complete(cycle.frames)
    }
    fn write_sink(
        &mut self,
        binding: &ExternalBinding,
        source: &[f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        if let Some(result) = self
            .bridged
            .and_then(|(devices, host)| devices.write(binding.node_id().as_str(), source, host))
        {
            return result;
        }
        let Some(map) = self
            .bindings
            .iter()
            .find(|map| !map.input && map.id == binding.node_id().as_str())
        else {
            return TransferOutcome::complete(0);
        };
        // Multiple graph sinks may map to the same physical channels: mix their
        // contributions into the callback's initially zeroed output, not overwrite.
        self.buffers.write(map, self.offset, source);
        TransferOutcome::complete(cycle.frames)
    }
}

struct Renderer<'a> {
    timebase: mixrelay_realtime::Timebase,
    processor: &'a mut Box<BlockProcessor>,
    activation: Option<GraphActivation<'a>>,
    bindings: &'a [Binding],
    clock_id: u32,
    epoch: u64,
    native_clock: Option<(&'a DirectPlan, u64)>,
    status: &'a mixrelay_engine::session::AdapterRuntimeStatus,
    /// The first frame expected next, in the timeline last used; `None` restarts it.
    position: Option<u64>,
    progress: &'a AtomicU64,
    failed: &'a AtomicBool,
    invalid: &'a AtomicBool,
}

/// The installed native geometry is immutable throughout a direct lifetime.
/// A compatible DSP replacement can use it without withdrawing the callback or
/// rebuilding independent-clock converters. Other plans remain queued for rebind.
struct GraphActivation<'a> {
    installed: &'a RenderPlan,
    context: AudioContext<'a>,
    rebind: &'a AtomicBool,
}

impl GraphActivation<'_> {
    fn activate(&self, processor: &mut Box<BlockProcessor>) -> bool {
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
                        self.rebind.store(true, Ordering::Release);
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
unsafe extern "C" fn render(
    context: *mut c_void,
    input: *const c_void,
    output: *mut c_void,
    frames: u32,
    timing: *const mixrelay_macos_transport::device_io::IoTiming,
) {
    // SAFETY: the descriptor is published to exactly one serialized IOProc;
    // control never accesses this renderer until removal + reader drain.
    let renderer = unsafe { &mut *context.cast::<Renderer<'_>>() };
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        renderer.render(input, output, frames, timing.as_ref())
    }));
    if !matches!(result, Ok(true)) {
        renderer.failed.store(true, Ordering::Release);
    }
}
impl Renderer<'_> {
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
    unsafe fn render(
        &mut self,
        input: *const c_void,
        output: *mut c_void,
        frames: u32,
        timing: Option<&mixrelay_macos_transport::device_io::IoTiming>,
    ) -> bool {
        if self.failed.load(Ordering::Acquire) || frames == 0 || frames > 4096 {
            return false;
        }
        if self
            .activation
            .as_ref()
            .is_some_and(|activation| !activation.activate(self.processor))
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
            self.invalid.store(true, Ordering::Release);
            true
        })
    }
    fn render_buffers(
        &mut self,
        buffers: Buffers<'_>,
        frames: u32,
        timing: Option<&mixrelay_macos_transport::device_io::IoTiming>,
    ) -> bool {
        if !self.bindings.iter().all(|binding| buffers.covers(binding)) {
            self.invalid.store(true, Ordering::Release);
            return true;
        }
        let format = self.processor.plan().format();
        let mut io = CallbackIo {
            buffers,
            bindings: self.bindings,
            offset: 0,
            bridged: self
                .native_clock
                .as_ref()
                .map(|(plan, _)| (&plan.bridged, plan.callback_host())),
        };
        // Native sample time when the device reports it, otherwise continue the previous
        // position. Any jump (a device skip, or switching between the two) is declared a
        // discontinuity instead of failing the renderer.
        let time = timing.map(|timing| {
            self.native_clock
                .as_ref()
                .map_or(timing.output, |(plan, _)| plan.cycle_time(timing))
        });
        let native = time.and_then(native_sample);
        let base = native.or(self.position).unwrap_or(0);
        let mut discontinuity = self.position != Some(base);
        // A larger native quantum is split, never truncated or queued for later.
        while io.offset < frames as usize {
            let count = u32::try_from(frames as usize - io.offset)
                .map_or(format.block_frames, |remaining| {
                    remaining.min(format.block_frames)
                });
            let cycle = ProcessCycle {
                clock: ClockDomain {
                    authority: ClockAuthority::Device,
                    id: u64::from(self.clock_id),
                    epoch: self.epoch,
                },
                sample_rate: format.sample_rate,
                frames: count,
                first_frame: base + io.offset as u64,
                monotonic_ns: time.filter(|time| time.flags & 2 != 0).map(|time| {
                    self.timebase.nanoseconds(time.host_time)
                        + io.offset as u64 * 1_000_000_000 / u64::from(format.sample_rate)
                }),
                discontinuity: std::mem::take(&mut discontinuity),
            };
            let Ok(Some(report)) = self.processor.process_external_cycle(cycle, &mut io) else {
                return false;
            };
            if let Some((plan, _)) = &self.native_clock {
                plan.record_latency(self.processor, report, timing.copied(), cycle);
            }
            super::runtime_latency::record_sources(
                self.processor,
                super::endpoint_service::source_registry(),
                true,
                cycle.clock.epoch,
            );
            self.status.record_process(report);
            io.offset += count as usize;
        }
        self.position = Some(base + u64::from(frames));
        let synchronized = self.native_clock.as_ref().is_none_or(|(plan, _)| {
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
        });
        self.status
            .observe_clock(ClockAuthority::Device, synchronized);
        self.progress.fetch_add(1, Ordering::Release);
        true
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn native_sample(time: mixrelay_macos_transport::device_io::IoTimestamp) -> Option<u64> {
    (time.flags & 1 != 0
        && time.sample_time.is_finite()
        && time.sample_time >= 0.0
        && time.sample_time < 9_007_199_254_740_992.0)
        .then_some(time.sample_time as u64)
}

fn log_route(plan: &DirectPlan, rate: u32, buffer: u32) {
    tracing::info!(
        clock_id = plan.clock_id,
        processing_rate = rate,
        buffer_frames = buffer,
        bindings = plan.bindings.len(),
        "CoreAudio direct renderer starting"
    );
}

#[derive(Default)]
struct RenderSignals {
    progress: AtomicU64,
    failed: AtomicBool,
    invalid: AtomicBool,
    rebind: AtomicBool,
}

pub(super) fn run(
    plan: &DirectPlan,
    processor: &mut Box<BlockProcessor>,
    context: AudioContext<'_>,
) -> Result<(), String> {
    let signals = RenderSignals::default();
    let installed_plan = std::sync::Arc::clone(processor.plan());
    let graph_rate = processor.plan().format().sample_rate;
    let renderer = Renderer {
        timebase: mixrelay_realtime::Timebase::current(),
        processor,
        activation: Some(GraphActivation {
            installed: &installed_plan,
            context,
            rebind: &signals.rebind,
        }),
        bindings: &plan.bindings,
        clock_id: plan.clock_id,
        epoch: super::super::macos_devices::direct::next_epoch(),
        native_clock: Some((plan, plan.discontinuities())),
        status: context.runtime_status,
        position: None,
        progress: &signals.progress,
        failed: &signals.failed,
        invalid: &signals.invalid,
    };
    log_route(
        plan,
        graph_rate,
        renderer.processor.plan().format().block_frames,
    );
    let mut prepared = PreparedRenderer::new(renderer);
    let installed = prepared.install(plan);
    let mut last_execution_report = Instant::now();
    signals.supervise(plan, context, &mut last_execution_report);
    drop(installed);
    plan.report_execution(last_execution_report.elapsed());
    finish_render(
        plan,
        context,
        signals.invalid.load(Ordering::Acquire),
        signals.failed.load(Ordering::Acquire),
    )
}

impl RenderSignals {
    fn supervise(
        &self,
        plan: &DirectPlan,
        context: AudioContext<'_>,
        last_execution_report: &mut Instant,
    ) {
        let mut ticks = 0;
        let mut last = Instant::now();
        while !context.runtime_status.stop_requested()
            && !context.runtime_status.pause_requested()
            && !self.rebind.load(Ordering::Acquire)
            && !context.devices.has_pending()
            && plan.healthy()
        {
            if self.failed.load(Ordering::Acquire) || self.invalid.load(Ordering::Acquire) {
                break;
            }
            if last_execution_report.elapsed() >= Duration::from_secs(5) {
                plan.report_execution(last_execution_report.elapsed());
                *last_execution_report = Instant::now();
            }
            let next = self.progress.load(Ordering::Acquire);
            if next != ticks {
                ticks = next;
                last = Instant::now();
            } else if last.elapsed() > Duration::from_secs(2) {
                tracing::warn!(
                    clock_id = plan.clock_id,
                    "CoreAudio direct callback progress stalled"
                );
                break;
            }
            // Supervisory responsiveness only. This thread never moves PCM; every
            // block has already been rendered synchronously inside the IOProc.
            thread::sleep(Duration::from_millis(1));
        }
    }
}

fn finish_render(
    plan: &DirectPlan,
    context: AudioContext<'_>,
    invalid: bool,
    failed: bool,
) -> Result<(), String> {
    tracing::info!(
        clock_id = plan.clock_id,
        invalid_layout = invalid,
        render_failed = failed,
        stop = context.runtime_status.stop_requested(),
        pause = context.runtime_status.pause_requested(),
        graph_pending = context.exchange.has_pending(),
        devices_pending = context.devices.has_pending(),
        healthy = plan.healthy(),
        "CoreAudio direct renderer stopped"
    );
    if invalid {
        tracing::warn!(
            clock_id = plan.clock_id,
            "CoreAudio callback rejected native buffer layout"
        );
        plan.request_reconnect();
    }
    if !plan.healthy() {
        context.devices.request_recovery();
    }
    if failed {
        return Err("CoreAudio direct callback rejected buffer geometry or DSP processing".into());
    }
    Ok(())
}

/// Runs `drive` while `plan`'s master callback renders `processor`, installed as `run`
/// installs it, without `CoreAudio` or supervision.
#[cfg(test)]
pub(crate) fn render_directly<R>(
    plan: &DirectPlan,
    processor: &mut Box<BlockProcessor>,
    drive: impl FnOnce() -> R,
) -> R {
    let status = mixrelay_engine::session::AdapterRuntimeStatus::new(
        super::adapter_profile(),
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    );
    let signals = RenderSignals::default();
    let mut prepared = PreparedRenderer::new(Renderer {
        timebase: mixrelay_realtime::Timebase::current(),
        processor,
        activation: None,
        bindings: &plan.bindings,
        clock_id: plan.clock_id,
        epoch: super::super::macos_devices::direct::next_epoch(),
        native_clock: Some((plan, plan.discontinuities())),
        status: &status,
        position: None,
        progress: &signals.progress,
        failed: &signals.failed,
        invalid: &signals.invalid,
    });
    let installed = prepared.install(plan);
    let result = drive();
    drop(installed);
    assert!(
        !signals.failed.load(Ordering::Acquire) && !signals.invalid.load(Ordering::Acquire),
        "direct callback rejected its buffers"
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixrelay_audio_graph::{
        CompileContext, CompileLimits, GraphCompiler, GraphDocument, RenderFormat,
    };
    use mixrelay_engine::RuntimeStatusBackend;
    use std::sync::Arc;

    pub(super) fn stereo_bindings() -> [Binding; 2] {
        [
            Binding {
                id: "input".into(),
                input: true,
                offset: Some(0),
                channels: 2,
            },
            Binding {
                id: "output".into(),
                input: false,
                offset: Some(0),
                channels: 2,
            },
        ]
    }

    pub(super) fn stereo_list(pcm: &mut [f32]) -> AudioBufferList {
        AudioBufferList {
            count: 1,
            first: AudioBuffer {
                channels: 2,
                byte_size: u32::try_from(std::mem::size_of_val(pcm)).unwrap(),
                data: pcm.as_mut_ptr(),
            },
        }
    }

    pub(super) fn processor_at(block_frames: u32, sample_rate: u32) -> Box<BlockProcessor> {
        let document: GraphDocument = serde_json::from_value(serde_json::json!({
            "schemaVersion":2,"revision":1,"nodes":[
                {"id":"input","type":"audio_device_input","config":{"selector":{"match":"default_input"},"outputChannels":2,"gainDb":0}},
                {"id":"output","type":"audio_device_output","config":{"selector":{"match":"default_output"},"inputChannels":2}}
            ],"edges":[{"id":"wire","source":{"nodeId":"input","portId":"audio"},"destination":{"nodeId":"output","portId":"audio"}}]
        })).unwrap();
        let execution = GraphCompiler::default()
            .compile(
                &document,
                CompileContext {
                    format: RenderFormat {
                        sample_rate,
                        block_frames,
                    },
                    limits: CompileLimits::default(),
                },
            )
            .unwrap();
        Box::new(BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap())
    }

    fn offline_branch_processor() -> Box<BlockProcessor> {
        let document: GraphDocument = serde_json::from_value(serde_json::json!({
            "schemaVersion":2,"revision":1,"nodes":[
                {"id":"input","type":"audio_device_input","config":{"selector":{"match":"default_input"},"outputChannels":2,"gainDb":0}},
                {"id":"missing-input","type":"audio_device_input","config":{"selector":{"match":"default_input"},"outputChannels":2,"gainDb":0}},
                {"id":"output","type":"audio_device_output","config":{"selector":{"match":"default_output"},"inputChannels":2}},
                {"id":"missing-output","type":"audio_device_output","config":{"selector":{"match":"default_output"},"inputChannels":2}}
            ],"edges":[
                {"id":"live","source":{"nodeId":"input","portId":"audio"},"destination":{"nodeId":"output","portId":"audio"}},
                {"id":"silent","source":{"nodeId":"missing-input","portId":"audio"},"destination":{"nodeId":"output","portId":"audio"}},
                {"id":"discard","source":{"nodeId":"input","portId":"audio"},"destination":{"nodeId":"missing-output","portId":"audio"}}
            ]
        })).unwrap();
        let execution = GraphCompiler::default()
            .compile(
                &document,
                CompileContext {
                    format: RenderFormat {
                        sample_rate: 48_000,
                        block_frames: 32,
                    },
                    limits: CompileLimits::default(),
                },
            )
            .unwrap();
        Box::new(BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap())
    }

    #[test]
    fn offline_transfers_are_inactive_silence_and_discard_with_complete_frame_counts() {
        let processor = processor_at(32, 48_000);
        let mut bindings = stereo_bindings();
        for binding in &mut bindings {
            binding.offset = None;
        }
        let mut io = CallbackIo {
            buffers: Buffers::empty(),
            bindings: &bindings,
            offset: 0,
            bridged: None,
        };
        assert!(bindings.iter().all(|binding| io.buffers.covers(binding)));
        let cycle = ProcessCycle {
            clock: ClockDomain {
                authority: ClockAuthority::Device,
                id: 7,
                epoch: 3,
            },
            sample_rate: 48_000,
            frames: 32,
            first_frame: 0,
            monotonic_ns: None,
            discontinuity: true,
        };
        // The engine hands every source a zeroed buffer; offline input leaves it silent.
        let mut pcm = [0.0; 64];
        let input = io.read_source(&processor.plan().source_bindings()[0], &mut pcm, &cycle);
        assert_eq!(
            input,
            TransferOutcome {
                activity: InputActivity::Inactive,
                ..TransferOutcome::complete(32)
            }
        );
        assert_eq!(pcm.map(f32::to_bits), [0.0_f32; 64].map(f32::to_bits));
        assert_eq!(
            io.write_sink(&processor.plan().sink_bindings()[0], &[0.5; 64], &cycle),
            TransferOutcome::complete(32)
        );
    }

    #[test]
    fn missing_device_branches_do_not_interrupt_live_callback_pcm_or_create_xruns() {
        let mut processor = offline_branch_processor();
        let mut bindings = Vec::from(stereo_bindings());
        for (id, input) in [("missing-input", true), ("missing-output", false)] {
            bindings.push(Binding {
                id: id.into(),
                input,
                offset: None,
                channels: 2,
            });
        }
        let status = mixrelay_engine::session::AdapterRuntimeStatus::new(
            crate::macos::adapter_profile(),
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        );
        let progress = AtomicU64::new(0);
        let failed = AtomicBool::new(false);
        let invalid = AtomicBool::new(false);
        let mut renderer = Renderer {
            timebase: mixrelay_realtime::Timebase::current(),
            processor: &mut processor,
            activation: None,
            bindings: &bindings,
            clock_id: 7,
            epoch: 3,
            native_clock: None,
            status: &status,
            position: None,
            progress: &progress,
            failed: &failed,
            invalid: &invalid,
        };
        exercise_live_callback(&mut renderer);
        assert_eq!(progress.load(Ordering::Acquire), 10);
        assert!(!invalid.load(Ordering::Acquire));
        assert!(!failed.load(Ordering::Acquire));
        let measured = status.status(RenderFormat {
            sample_rate: 48_000,
            block_frames: 32,
        });
        assert_eq!((measured.underruns, measured.overruns), (0, 0));
    }

    fn exercise_live_callback(renderer: &mut Renderer<'_>) {
        let mut input = [0.25; 98];
        let mut output = [0.0; 98];
        let input_list = stereo_list(&mut input);
        let mut output_list = stereo_list(&mut output);
        for _ in 0..10 {
            output.fill(0.0);
            // SAFETY: both ABLs own 49 stereo frames for this synchronous call;
            // the two offline maps have no native storage to dereference.
            assert!(unsafe {
                renderer.render(
                    (&raw const input_list).cast(),
                    (&raw mut output_list).cast(),
                    49,
                    None,
                )
            });
            assert_eq!(output.map(f32::to_bits), input.map(f32::to_bits));
        }
    }

    #[test]
    fn switching_between_fallback_and_native_timestamps_never_stops_rendering() {
        let mut processor = processor_at(32, 48000);
        let status = mixrelay_engine::session::AdapterRuntimeStatus::new(
            crate::macos::adapter_profile(),
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        );
        let progress = AtomicU64::new(0);
        let failed = AtomicBool::new(false);
        let invalid = AtomicBool::new(false);
        let bindings = stereo_bindings();
        let mut renderer = Renderer {
            timebase: mixrelay_realtime::Timebase::current(),
            processor: &mut processor,
            activation: None,
            bindings: &bindings,
            clock_id: 7,
            epoch: 3,
            native_clock: None,
            status: &status,
            position: None,
            progress: &progress,
            failed: &failed,
            invalid: &invalid,
        };
        let mut input = vec![0.25; 64];
        let mut output = vec![0.0; 64];
        let input_list = stereo_list(&mut input);
        let mut output_list = stereo_list(&mut output);
        let native = |sample_time: f64| mixrelay_macos_transport::device_io::IoTiming {
            now: mixrelay_macos_transport::device_io::IoTimestamp::default(),
            input: mixrelay_macos_transport::device_io::IoTimestamp {
                sample_time,
                host_time: 0,
                rate_scalar: 1.0,
                flags: 0,
            },
            output: mixrelay_macos_transport::device_io::IoTimestamp {
                sample_time,
                host_time: 1_000,
                rate_scalar: 1.0,
                flags: 3,
            },
        };
        // Missing, native far ahead, continuing, missing again, then a device skip.
        let timings = [
            None,
            Some(native(96_000.0)),
            Some(native(96_032.0)),
            None,
            Some(native(200_000.0)),
        ];
        let expected = [32, 96_032, 96_064, 96_096, 200_032];
        for (timing, position) in timings.iter().zip(expected) {
            assert!(unsafe {
                renderer.render(
                    (&raw const input_list).cast(),
                    (&raw mut output_list).cast(),
                    32,
                    timing.as_ref(),
                )
            });
            assert_eq!(renderer.position, Some(position));
        }
        assert!(!failed.load(Ordering::Acquire));
        assert_eq!(progress.load(Ordering::Acquire), 5);
    }

    #[test]
    fn synchronous_callback_preserves_every_sample_at_all_product_buffers() {
        for &block_frames in mixrelay_audio_io::formats::BUFFER_SIZES {
            let mut processor = processor_at(block_frames, 48000);
            let status = mixrelay_engine::session::AdapterRuntimeStatus::new(
                crate::macos::adapter_profile(),
                std::sync::Arc::new(mixrelay_time::ManualClock::default()),
            );
            let progress = AtomicU64::new(0);
            let failed = AtomicBool::new(false);
            let invalid = AtomicBool::new(false);
            let bindings = stereo_bindings();
            let mut renderer = Renderer {
                timebase: mixrelay_realtime::Timebase::current(),
                processor: &mut processor,
                activation: None,
                bindings: &bindings,
                clock_id: 7,
                epoch: 3,
                native_clock: None,
                status: &status,
                position: None,
                progress: &progress,
                failed: &failed,
                invalid: &invalid,
            };
            // Native callback larger than the DSP quantum proves split offsets,
            // including non-divisible final blocks. No queued next-call output.
            let frames = (block_frames + 17).min(4096);
            let mut input = vec![0.0; frames as usize * 2];
            let mut output = vec![0.0; input.len()];
            let input_list = stereo_list(&mut input);
            let mut output_list = stereo_list(&mut output);
            for cycle in 0..100 {
                for (index, sample) in input.iter_mut().enumerate() {
                    *sample = f32::from(
                        u16::try_from((cycle * input_list.first.byte_size as usize + index) % 8192)
                            .unwrap(),
                    ) / 8192.0;
                }
                output.fill(0.0);
                assert!(unsafe {
                    renderer.render(
                        (&raw const input_list).cast(),
                        (&raw mut output_list).cast(),
                        frames,
                        None,
                    )
                });
                assert_eq!(input, output, "buffer={block_frames} cycle={cycle}");
            }
            assert_eq!(progress.load(Ordering::Acquire), 100);
            assert_eq!(renderer.position, Some(u64::from(frames) * 100));
            let measured = status.status(RenderFormat {
                sample_rate: 48000,
                block_frames,
            });
            assert_eq!(measured.underruns, 0);
            assert_eq!(measured.overruns, 0);
        }
    }

    #[test]
    fn disabled_unused_stream_keeps_offsets_but_selected_missing_pcm_is_rejected() {
        #[repr(C)]
        struct List {
            count: u32,
            buffers: [AudioBuffer; 2],
        }
        let mut input = [0.25_f32; 64];
        let list = List {
            count: 2,
            buffers: [
                AudioBuffer {
                    channels: 3,
                    byte_size: 0,
                    data: std::ptr::null_mut(),
                },
                AudioBuffer {
                    channels: 2,
                    byte_size: 256,
                    data: input.as_mut_ptr(),
                },
            ],
        };
        let map = Binding {
            id: "input".into(),
            input: true,
            offset: Some(3),
            channels: 2,
        };
        let mut destination = [0.0; 64];
        // SAFETY: sized list and live PCM are retained across this operation;
        // inactive metadata has no PCM and destination is separate scratch.
        unsafe {
            Buffers::with_native((&raw const list).cast(), std::ptr::null_mut(), 32, |view| {
                assert!(view.covers(&map));
                assert!(!view.covers(&Binding {
                    id: "missing".into(),
                    input: true,
                    offset: Some(2),
                    channels: 2,
                }));
                view.transfer(&map, 0, &mut destination);
            })
        }
        .unwrap();
        assert_eq!(destination.map(f32::to_bits), input.map(f32::to_bits));
        // SAFETY: metadata is live; the advertised PCM byte count is deliberately
        // inconsistent with 64 frames and rejected before a PCM reference exists.
        assert!(
            unsafe {
                Buffers::with_native((&raw const list).cast(), std::ptr::null_mut(), 64, |_| ())
            }
            .is_none()
        );
    }
}

#[cfg(test)]
#[path = "direct_mixed_tests.rs"]
mod mixed_tests;

#[cfg(test)]
#[path = "direct/measurement.rs"]
mod measurement;
