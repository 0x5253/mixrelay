//! Owns the graph worker, its capture timeline and block-by-block DSP handoff.
use super::{direct, external_io::MacEngineExternalIo, runtime_latency, worker_runtime};
use crate::macos_pacing::{Pace, Publication};
use mixrelay_audio_engine::{BlockProcessor, ExternalIo, ProcessorExchange};
use mixrelay_audio_io::process::{ProcessCycle, ProcessReport};
use mixrelay_engine::session::AdapterRuntimeStatus;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

mod pacing;
pub(super) use pacing::{NativePacers, RouteClock, next_native_pace};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("macOS processing admission failed: {0}")]
    ProcessingAdmission(&'static str),
    #[error("macOS device activation failed: {0}")]
    DeviceActivation(&'static str),
    #[error("macOS direct renderer failed: {0}")]
    Direct(String),
    #[error("macOS graph cycle failed: {0}")]
    Process(#[from] mixrelay_audio_engine::ProcessBlockError),
}

/// Starts the sole native graph worker and its completion observation.
/// # Errors
/// Returns an operating-system thread creation failure before publishing a worker.
pub fn start(
    processor: Box<BlockProcessor>,
    exchange: Arc<ProcessorExchange>,
    devices: Arc<crate::macos_devices::MacDevices>,
    status: Arc<AdapterRuntimeStatus>,
) -> std::io::Result<(
    mixrelay_engine_runtime::AudioWorker,
    super::worker_completion::Completion<Error>,
)> {
    let (finished, completion) = super::worker_completion::channel();
    let worker_status = status.clone();
    let thread = std::thread::Builder::new()
        .name("mixrelay-audio-worker".into())
        .spawn(move || {
            let result = run_audio_worker(
                processor,
                AudioContext {
                    exchange: &exchange,
                    runtime_status: &worker_status,
                    devices: &devices,
                },
            );
            let _ = finished.send(result);
        })?;
    Ok((
        mixrelay_engine_runtime::AudioWorker::new(status, thread),
        completion,
    ))
}

#[derive(Clone, Copy)]
pub(super) struct AudioContext<'a> {
    pub(super) exchange: &'a ProcessorExchange,
    pub(super) runtime_status: &'a AdapterRuntimeStatus,
    pub(super) devices: &'a crate::macos_devices::MacDevices,
}
impl AudioContext<'_> {
    pub(super) fn activate(
        &self,
        processor: &mut Box<BlockProcessor>,
        active_devices: &mut crate::macos_devices::DeviceSet,
        activation: &mut crate::macos_devices::DeviceActivation<'_>,
    ) -> bool {
        // Same-spec native recovery must not wait for an unrelated graph edit.
        activation.activate_recovered(active_devices);
        match self.exchange.activate_pending_configured(processor, |_| {
            activation.activate(active_devices);
            Ok::<(), std::convert::Infallible>(())
        }) {
            Ok(activated) => activated,
            Err(never) => match never {},
        }
    }
}
fn run_audio_worker(
    mut processor: Box<BlockProcessor>,
    context: AudioContext<'_>,
) -> Result<(), Error> {
    let runtime_status = context.runtime_status;
    let _lease = runtime_status
        .begin_processing()
        .map_err(Error::ProcessingAdmission)?;
    let mut activation = context
        .devices
        .activation()
        .map_err(Error::DeviceActivation)?;
    let mut active_devices = crate::macos_devices::DeviceSet::default();
    let mut timeline = MacTimeline::new();
    let mut pacers = NativePacers::default();
    let mut routes = pacing::PlanRoutes::new(crate::endpoint_service::virtual_source_registry());
    let mut awaiting_format = false;
    let mut runtime = worker_runtime::WorkerRuntime::new(context.devices.device_wakeup());
    // Whether a direct (IOProc-rendered) plan must be looked for again. Looking allocates
    // and queries devices, so it happens off realtime and only after something changed,
    // or once a second in case a device became eligible on its own.
    let mut direct_recheck = true;
    let mut direct_checked = Instant::now();
    let result = (|| {
        runtime.off_realtime(|| routes.activate(processor.plan()));
        while !runtime_status.stop_requested() {
            if runtime_status.checkpoint() {
                timeline.reset();
                pacers = NativePacers::default();
                runtime.off_realtime(|| std::thread::sleep(Duration::from_millis(1)));
                timeline.deadline = Instant::now();
                continue;
            }
            // Activation is a lock-free handoff of prepared plans and device sets (atomic
            // checks, then pointer swaps; retirement happens elsewhere), so it stays in the
            // realtime class.
            if context.activate(&mut processor, &mut active_devices, &mut activation) {
                timeline.reset();
                pacers = NativePacers::default();
                runtime.off_realtime(|| routes.activate(processor.plan()));
                direct_recheck = true;
            }
            let format = processor.plan().format();
            if !active_devices.processing_ready(format.sample_rate) {
                log_format_gate(&mut runtime, &mut awaiting_format, true, format.sample_rate);
                runtime_status.observe_clock_idle();
                timeline.reset();
                pacers = NativePacers::default();
                direct_recheck = true;
                runtime.off_realtime(|| std::thread::sleep(Duration::from_millis(1)));
                continue;
            }
            log_format_gate(
                &mut runtime,
                &mut awaiting_format,
                false,
                format.sample_rate,
            );
            runtime_status.activate_format(format);
            if direct_recheck || direct_checked.elapsed() >= Duration::from_secs(1) {
                direct_recheck = false;
                direct_checked = Instant::now();
                if run_direct(&mut runtime, &active_devices, &mut processor, context)? {
                    timeline.reset();
                    pacers = NativePacers::default();
                    direct_recheck = true;
                    continue;
                }
            }
            process_worker_block(
                &mut timeline,
                &mut pacers,
                &mut WorkerBlock {
                    processor: &mut processor,
                    devices: &active_devices,
                    status: runtime_status,
                    runtime: &mut runtime,
                },
            )?;
        }
        Ok(())
    })();
    runtime.finish();
    result
}

/// Logs entering and leaving the device-format gate once per change, off realtime.
fn log_format_gate(
    runtime: &mut worker_runtime::WorkerRuntime,
    awaiting: &mut bool,
    gated: bool,
    processing_rate: u32,
) {
    if std::mem::replace(awaiting, gated) == gated {
        return;
    }
    runtime.off_realtime(|| {
        if gated {
            tracing::info!(
                processing_rate,
                "CoreAudio rendering gated until device format is ready"
            );
        } else {
            tracing::info!(
                processing_rate,
                "CoreAudio device format ready; rendering admitted"
            );
        }
    });
}

/// Hands rendering to a device `IOProc` when a direct plan exists, and supervises it until
/// it ends. Looking for, running and dropping the plan all happen off realtime.
fn run_direct(
    runtime: &mut worker_runtime::WorkerRuntime,
    devices: &crate::macos_devices::DeviceSet,
    processor: &mut Box<BlockProcessor>,
    context: AudioContext<'_>,
) -> Result<bool, Error> {
    let plan = runtime.off_realtime(|| {
        crate::macos_devices::direct::DirectPlan::prepare(devices, processor.plan())
    });
    let Some(plan) = plan else {
        return Ok(false);
    };
    runtime.record(worker_runtime::Path::Direct);
    runtime
        .off_realtime(|| {
            let result = direct::run(&plan, processor, context);
            drop(plan);
            result
        })
        .map_err(Error::Direct)?;
    Ok(true)
}

struct WorkerBlock<'a> {
    processor: &'a mut BlockProcessor,
    devices: &'a crate::macos_devices::DeviceSet,
    status: &'a AdapterRuntimeStatus,
    runtime: &'a mut worker_runtime::WorkerRuntime,
}

fn process_worker_block(
    timeline: &mut MacTimeline,
    pacers: &mut NativePacers,
    block: &mut WorkerBlock<'_>,
) -> Result<(), Error> {
    let route = RouteClock::observe(block.processor, block.devices);
    let (pace, clock_id) = next_native_pace(
        pacers,
        block.devices,
        block.processor.plan(),
        mixrelay_audio_io::retention::RING_FRAMES,
    );
    let Some((frames, discontinuity)) = timeline.accept((pace, clock_id), &route, block.status)
    else {
        // Waking this thread creates no credit; only the selected native clock
        // advances the processor's device-owned frame coordinates.
        block.runtime.wait_for_credit(clock_id);
        return Ok(());
    };
    let format = block.processor.plan().format();
    let period = Duration::from_secs_f64(f64::from(frames) / f64::from(format.sample_rate));
    // The scheduling budget follows the wake cadence, not this (possibly partial) block:
    // once per device callback under a device clock, once per graph block otherwise.
    let block_period =
        Duration::from_secs_f64(f64::from(format.block_frames) / f64::from(format.sample_rate));
    let wake_period = (clock_id == worker_runtime::HARDWARE_CLOCK)
        .then(|| block.devices.hardware_clock_progress())
        .flatten()
        .filter(|hardware| hardware.native_rate != 0 && hardware.quantum != 0)
        .map_or(block_period, |hardware| {
            Duration::from_secs_f64(f64::from(hardware.quantum) / f64::from(hardware.native_rate))
        });
    block.runtime.render_at(wake_period);
    block.runtime.record(match clock_id {
        worker_runtime::HARDWARE_CLOCK => worker_runtime::Path::Hardware,
        worker_runtime::SHARED_SOURCE_CLOCK => worker_runtime::Path::SharedSource,
        _ => worker_runtime::Path::Timer,
    });
    let mut io = timeline.take_external_io(block.devices);
    io.head = pacers.head();
    let result = process_native_cycle(
        block.processor,
        &mut io,
        WorkerCycle {
            timeline,
            frames,
            discontinuity: discontinuity || timeline.discontinuity_pending,
        },
    );
    timeline.capture_alignment = io.alignment;
    block.runtime.record_time_reads(io.time_reads);
    let report = result?;
    block
        .devices
        .record_latency_cycle(block.processor, report, timeline.epoch);
    runtime_latency::record_sources(
        block.processor,
        io.sources,
        report.is_some(),
        timeline.epoch,
    );
    timeline.discontinuity_pending = report.is_none();
    if let Some(report) = report {
        block.status.record_process(report);
    }
    timeline.frame += u64::from(frames);
    if !timeline.native {
        wait_for_block(&mut timeline.deadline, period);
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) struct WorkerCycle<'a> {
    pub(super) timeline: &'a MacTimeline,
    pub(super) frames: u32,
    pub(super) discontinuity: bool,
}

pub(super) struct MacTimeline {
    pub(super) capture_alignment: mixrelay_audio_io::capture_alignment::CaptureAlignment,
    pub(super) frame: u64,
    pub(super) available_frames: u32,
    /// The pacing source publication that granted `available_frames`.
    pub(super) publication: Option<Publication>,
    pub(super) epoch: u64,
    pub(super) deadline: Instant,
    pub(super) native: bool,
    pub(super) clock_id: u64,
    pub(super) discontinuity_pending: bool,
}

impl MacTimeline {
    /// Transfers acquisition history and the complete pending credit into one graph read.
    pub(super) fn take_external_io<'a>(
        &mut self,
        devices: &'a crate::macos_devices::DeviceSet,
    ) -> MacEngineExternalIo<'a> {
        let mut io = MacEngineExternalIo::new(devices);
        io.capture_frames = self.available_frames;
        io.publication = self.publication;
        io.alignment = std::mem::take(&mut self.capture_alignment);
        io
    }

    pub(super) fn new() -> Self {
        Self {
            capture_alignment: mixrelay_audio_io::capture_alignment::CaptureAlignment::default(),
            frame: 0,
            available_frames: 0,
            publication: None,
            epoch: 0,
            deadline: Instant::now(),
            native: false,
            clock_id: 0,
            discontinuity_pending: false,
        }
    }
    pub(super) fn reset(&mut self) {
        self.capture_alignment = mixrelay_audio_io::capture_alignment::CaptureAlignment::default();
        self.frame = 0;
        self.available_frames = 0;
        self.publication = None;
        self.epoch = self.epoch.wrapping_add(1);
        self.deadline = Instant::now();
    }

    pub(super) fn accept(
        &mut self,
        pacing: (Pace, u64),
        route: &RouteClock,
        status: &AdapterRuntimeStatus,
    ) -> Option<(u32, bool)> {
        let (pace, clock_id) = pacing;
        match pace {
            Pace::Block {
                first_frame,
                frames,
                available_frames,
                epoch,
                discontinuity,
                overrun,
                publication,
            } => {
                let switched = !self.native || self.clock_id != clock_id;
                self.native = true;
                self.clock_id = clock_id;
                self.frame = first_frame;
                self.available_frames = available_frames;
                self.publication = publication;
                self.epoch = epoch;
                if overrun {
                    status.record_process(ProcessReport {
                        output_overflows: 1,
                        discontinuities: 1,
                        ..ProcessReport::default()
                    });
                }
                status.observe_clock(
                    mixrelay_audio_io::session::ClockAuthority::Device,
                    route.synchronized,
                );
                Some((frames, discontinuity || switched))
            }
            Pace::Wait | Pace::RateMismatch => None,
            Pace::Idle | Pace::Unknown => {
                let switched = self.native;
                if switched {
                    self.reset();
                }
                self.native = false;
                self.clock_id = 0;
                self.available_frames = route.maximum_frames;
                self.publication = None;
                if route.independent_idle {
                    status.observe_clock_idle();
                } else {
                    status.observe_clock(
                        mixrelay_audio_io::session::ClockAuthority::EngineTimer,
                        false,
                    );
                }
                Some((route.maximum_frames, switched || self.frame == 0))
            }
        }
    }
}

fn wait_for_block(deadline: &mut Instant, period: Duration) {
    *deadline += period;
    let now = Instant::now();
    if *deadline > now {
        std::thread::sleep(deadline.duration_since(now));
    } else if now.duration_since(*deadline) > period.saturating_mul(4) {
        *deadline = now;
    }
}

pub(super) fn process_native_cycle(
    processor: &mut BlockProcessor,
    io: &mut impl ExternalIo,
    cycle: WorkerCycle<'_>,
) -> Result<Option<ProcessReport>, mixrelay_audio_engine::ProcessBlockError> {
    let format = processor.plan().format();
    let WorkerCycle {
        timeline,
        frames,
        discontinuity,
        ..
    } = cycle;
    let clock_id = timeline.clock_id;
    processor.process_external_cycle(
        ProcessCycle {
            clock: mixrelay_audio_io::process::ClockDomain {
                authority: if clock_id != 0 {
                    mixrelay_audio_io::session::ClockAuthority::Device
                } else {
                    mixrelay_audio_io::session::ClockAuthority::EngineTimer
                },
                id: clock_id,
                epoch: timeline.epoch,
            },
            sample_rate: format.sample_rate,
            frames,
            first_frame: timeline.frame,
            monotonic_ns: None,
            discontinuity,
        },
        io,
    )
}
