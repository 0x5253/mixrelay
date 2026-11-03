//! Retained atomic evidence; only the reporter acquires the control-side cache lock.
use super::{DeviceSet, MacDevices, device::Device, ffi};
use mixrelay_audio_engine::BlockProcessor;
#[cfg(test)]
use mixrelay_audio_io::residence::{EdgeSnapshot, PassageSnapshot};
use mixrelay_audio_io::{
    process::{ProcessCycle, ProcessReport},
    residence::{Residence, ResidenceSnapshot},
};
use mixrelay_macos_transport::device_io::{
    IoTiming, LatencySnapshot, latency_snapshot, record_direct_latency,
};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Default)]
pub(super) struct BridgeMeasurements {
    pub(super) src_input: Residence,
    pub(super) src_output: Residence,
    pub(super) src_delay: AtomicU64,
    pub(super) faults: AtomicU64,
    revision: AtomicU64,
    graph_epoch: AtomicU64,
    compensation_ready: AtomicBool,
}

#[derive(Clone, Copy)]
pub(crate) struct Observation {
    pub(crate) identity: u64,
    pub(crate) epoch: u64,
    pub(crate) graph_epoch: u64,
    pub(crate) generation: u64,
    pub(crate) clock_breaks: u64,
    pub(crate) native_rate: u32,
    pub(crate) graph_rate: u32,
    pub(crate) direct: bool,
    pub(crate) ready: bool,
    pub(crate) sampled: bool,
    pub(crate) faults: u64,
    pub(crate) native: LatencySnapshot,
    pub(crate) src_input: ResidenceSnapshot,
    pub(crate) src_output: ResidenceSnapshot,
    pub(crate) src_delay: u64,
}
impl Observation {
    pub(crate) fn context(self) -> (u64, u64, u64, u64, u32, u32, bool, u64) {
        (
            self.identity,
            self.epoch,
            self.generation,
            self.clock_breaks,
            self.native_rate,
            self.graph_rate,
            self.direct,
            self.graph_epoch,
        )
    }
    /// Why this window cannot be measured, for diagnostics; `None` when it is usable.
    pub(crate) fn instability(self, previous: Self) -> Option<&'static str> {
        if !self.sampled || !previous.sampled {
            Some("taps not sampled")
        } else if !self.ready {
            Some("not ready")
        } else if self.faults != previous.faults {
            Some("binding faults advanced")
        } else if self.native.faults != previous.native.faults {
            Some("native faults advanced")
        } else if self.native.native_queue.consumer.discarded
            != previous.native.native_queue.consumer.discarded
        {
            Some("native queue residence invalid")
        } else if self.native.graph_queue.consumer.discarded
            != previous.native.graph_queue.consumer.discarded
            && self
                .native
                .content_age
                .mean_since(previous.native.content_age)
                .is_none()
        {
            // Timestamped input reads may intentionally skip retained history.
            // Exact age still measures every transferred frame; output and
            // unknown-coordinate windows retain the queue-discard fence.
            Some("graph queue residence invalid")
        } else if self.src_input.invalid != previous.src_input.invalid
            || self.src_output.invalid != previous.src_output.invalid
        {
            Some("SRC residence invalid")
        } else {
            None
        }
    }
    /// Host intervals and native/graph FIFO passages in ticks; SRC means in their rate domain.
    pub(crate) fn delay_ns(self, previous: Self, input: bool, ns_per_tick: f64) -> Option<f64> {
        if input && !self.direct {
            let age = self.native.content_age;
            let before = previous.native.content_age;
            if age.weight != before.weight || age.invalid != before.invalid {
                // Each transferred frame retains its acquisition coordinate through
                // SRC. Its observed age includes all source queues/filter history;
                // summing separately averaged queue windows would count it again.
                return age.mean_since(before);
            }
        }
        let timing = if input {
            self.native
                .acquisition
                .mean_since(previous.native.acquisition)
        } else {
            self.native
                .presentation
                .mean_since(previous.native.presentation)
        }?;
        if self.direct {
            return Some(timing * ns_per_tick);
        }
        let native = self
            .native
            .native_queue
            .residence_since(previous.native.native_queue)?;
        let graph = self
            .native
            .graph_queue
            .residence_since(previous.native.graph_queue)?;
        let src_input = self.src_input.mean_since(previous.src_input)?;
        let src_output = self.src_output.mean_since(previous.src_output)?;
        let scalar = if input {
            self.native
                .input_scalar
                .mean_since(previous.native.input_scalar)
        } else {
            self.native
                .output_scalar
                .mean_since(previous.native.output_scalar)
        }
        .map_or(1.0, |scalar| scalar / 1e9);
        let native_frame_ns = scalar * 1e9 / f64::from(self.native_rate);
        let graph_frame_ns = 1e9 / f64::from(self.graph_rate);
        // SRC queue-time areas advance in nominal native callback time. Both
        // domains need its measured scalar, including graph-domain residence.
        // Algorithmic delay is a frame count at the converter's output rate.
        let (input_frame_ns, output_frame_ns, delay_frame_ns) = if input {
            (native_frame_ns, graph_frame_ns * scalar, graph_frame_ns)
        } else {
            (graph_frame_ns * scalar, native_frame_ns, native_frame_ns)
        };
        #[allow(clippy::cast_precision_loss)]
        let delay = self.src_delay as f64;
        Some(
            (timing + native + graph) * ns_per_tick
                + src_input * 0.5 * input_frame_ns
                + src_output * 0.5 * output_frame_ns
                + delay * delay_frame_ns,
        )
    }
}

impl DeviceSet {
    pub(crate) fn record_latency_cycle(
        &self,
        processor: &BlockProcessor,
        report: Option<ProcessReport>,
        epoch: u64,
    ) {
        let _ = processor.acknowledge_content_latency();
        for (id, device) in &self.0 {
            let ready = processor.binding_content_latency_ready(id, device.input);
            let taps = &device.native.latency;
            taps.compensation_ready.store(ready, Ordering::Release);
            taps.graph_epoch.store(epoch, Ordering::Release);
            taps.revision
                .store(processor.plan().revision(), Ordering::Release);
            if !ready || report.is_none() {
                taps.faults.fetch_add(1, Ordering::Release);
            }
        }
    }
}

impl super::direct::DirectPlan {
    /// Records one graph cycle of a direct quantum: its `cycle.frames` weight the
    /// quantum's timing, so the cycles of one split callback add up to its frames.
    pub(crate) fn record_latency(
        &self,
        processor: &BlockProcessor,
        report: ProcessReport,
        timing: Option<IoTiming>,
        cycle: ProcessCycle,
    ) {
        let epoch = cycle.clock.epoch;
        let _ = processor.acknowledge_content_latency();
        for owner in &self.owners {
            let ready = self
                .latency_bindings
                .iter()
                .filter(|(_, device)| std::sync::Arc::ptr_eq(device, owner))
                .all(|(id, device)| processor.binding_content_latency_ready(id, device.input));
            if let Some(timing) = timing {
                // SAFETY: this direct master is the sole writer of each direct tap.
                unsafe {
                    record_direct_latency(
                        owner.native.pointer.as_ptr().cast(),
                        timing,
                        cycle.frames,
                    );
                };
            }
            owner
                .native
                .latency
                .compensation_ready
                .store(ready, Ordering::Release);
            owner
                .native
                .latency
                .graph_epoch
                .store(epoch, Ordering::Release);
            owner
                .native
                .latency
                .revision
                .store(processor.plan().revision(), Ordering::Release);
            if !ready || timing.is_none() {
                owner.native.latency.faults.fetch_add(1, Ordering::Release);
            }
        }
        self.bridged
            .record_latency_cycle(processor, Some(report), epoch);
    }
}

/// Retained metadata that qualifies one device's tap snapshot.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Context {
    revision: u64,
    epoch: u64,
    graph_epoch: u64,
    generation: u64,
    clock_breaks: u64,
    native_rate: u32,
    graph_rate: u32,
    direct: bool,
    ready: bool,
    faults: u64,
    src_delay: u64,
}

impl Context {
    // Acquire pairs with each writer's Release store of the value read.
    fn read(device: &Device, timing_pending: bool) -> Self {
        let taps = &device.native.latency;
        let direct = device.native.render.enabled();
        let pointer = device.native.pointer.as_ptr();
        Self {
            revision: taps.revision.load(Ordering::Acquire),
            epoch: device.epoch.load(Ordering::Acquire),
            graph_epoch: taps.graph_epoch.load(Ordering::Acquire),
            generation: device.native.render.generation.load(Ordering::Acquire),
            // SAFETY: the caller's Arc retains the initialized owner; only atomics are read.
            clock_breaks: unsafe { ffi::mr_device_io_clock_discontinuities(pointer) },
            native_rate: device.native_rate.load(Ordering::Acquire),
            graph_rate: device.graph_rate.load(Ordering::Acquire),
            direct,
            ready: !timing_pending
                && !device.failed.load(Ordering::Acquire)
                && (direct || device.ready())
                && device.activity.ready()
                && taps.compensation_ready.load(Ordering::Acquire),
            faults: taps
                .faults
                .load(Ordering::Acquire)
                // SAFETY: as above.
                .wrapping_add(unsafe { ffi::mr_device_io_overflows(pointer) }),
            src_delay: taps.src_delay.load(Ordering::Acquire),
        }
    }
}

/// One coherent read of each cumulative tap, or `None` where a writer was busy.
struct Taps {
    native: Option<LatencySnapshot>,
    src_input: Option<ResidenceSnapshot>,
    src_output: Option<ResidenceSnapshot>,
}

fn read_taps(device: &Device, direct: bool) -> Taps {
    let taps = &device.native.latency;
    Taps {
        // SAFETY: the caller's Arc retains the initialized owner; only atomics are read.
        native: unsafe { latency_snapshot(device.native.pointer.as_ptr().cast(), direct) },
        src_input: taps.src_input.snapshot(),
        src_output: taps.src_output.snapshot(),
    }
}

/// Reads the context on both sides of the taps. A converter, owner or clock replaced
/// meanwhile would pair one context's cumulative taps with another's, and that pair
/// would then serve as the new context's baseline. Such a read is unsampled: its window
/// is skipped and, being unsampled, it is never a baseline.
fn observe(
    device: &Device,
    timing_pending: bool,
    taps: impl FnOnce(&Device, bool) -> Taps,
) -> Observation {
    let before = Context::read(device, timing_pending);
    let Taps {
        native,
        src_input,
        src_output,
    } = taps(device, before.direct);
    let context = Context::read(device, timing_pending);
    Observation {
        identity: device.identity,
        epoch: context.epoch,
        graph_epoch: context.graph_epoch,
        generation: context.generation,
        clock_breaks: context.clock_breaks,
        native_rate: context.native_rate,
        graph_rate: context.graph_rate,
        direct: context.direct,
        ready: context.ready,
        sampled: before == context
            && native.is_some()
            && src_input.is_some()
            && src_output.is_some(),
        faults: context.faults,
        native: native.unwrap_or_default(),
        src_input: src_input.unwrap_or_default(),
        src_output: src_output.unwrap_or_default(),
        src_delay: context.src_delay,
    }
}

impl MacDevices {
    pub(crate) fn latency_observations(
        &self,
        revision: u64,
        timing_pending: bool,
    ) -> BTreeMap<String, Observation> {
        let cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache
            .iter()
            .filter_map(|(id, (_, weak))| {
                let device = weak.upgrade()?;
                if device.native.latency.revision.load(Ordering::Acquire) != revision {
                    return None;
                }
                Some((id.clone(), observe(&device, timing_pending, read_taps)))
            })
            .collect()
    }
}

/// Match the same directional suffix contract as graph device lookup.
pub(crate) fn binding<'a>(
    observations: &'a BTreeMap<String, Observation>,
    id: &str,
    input: bool,
) -> Option<&'a Observation> {
    observations.get(id).or_else(|| {
        observations.get(&format!(
            "{id}/{}",
            if input { "capture" } else { "render" }
        ))
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn observation(direct: bool) -> Observation {
        Observation {
            identity: 1,
            epoch: 2,
            graph_epoch: 5,
            generation: 3,
            clock_breaks: 0,
            native_rate: 44_100,
            graph_rate: 48_000,
            direct,
            ready: true,
            sampled: true,
            faults: 0,
            native: LatencySnapshot::default(),
            src_input: ResidenceSnapshot::default(),
            src_output: ResidenceSnapshot::default(),
            src_delay: 0,
        }
    }
    use mixrelay_audio_engine::{
        ExternalBinding, ExternalIo, HostedAudioProcessor, HostedProcessorFactory,
        HostedProcessorPrepareRequest,
    };
    use mixrelay_audio_io::process::{ProcessCycle, TransferOutcome};
    pub(crate) struct Factory;
    impl HostedProcessorFactory for Factory {
        fn prepare(
            &self,
            _: HostedProcessorPrepareRequest<'_>,
        ) -> Result<Box<dyn HostedAudioProcessor>, String> {
            unreachable!()
        }
    }
    struct Io;
    impl ExternalIo for Io {
        fn read_source(
            &mut self,
            _: &ExternalBinding,
            _: &mut [f32],
            cycle: &ProcessCycle,
        ) -> TransferOutcome {
            TransferOutcome::complete(cycle.frames)
        }
        fn write_sink(
            &mut self,
            _: &ExternalBinding,
            _: &[f32],
            cycle: &ProcessCycle,
        ) -> TransferOutcome {
            TransferOutcome::complete(cycle.frames)
        }
    }
    #[test]
    fn unrelated_process_report_faults_do_not_taint_bridged_or_direct_bindings() {
        use mixrelay_audio_engine::PeakMeterBank;
        use mixrelay_audio_io::process::ClockDomain;
        use std::sync::Arc;
        let mut processor =
            crate::macos::test_feedback_processor(&Factory, Arc::new(PeakMeterBank::new(8, 64)));
        let cycle = ProcessCycle {
            clock: ClockDomain {
                authority: mixrelay_audio_io::session::ClockAuthority::Device,
                id: 1,
                epoch: 1,
            },
            sample_rate: 48_000,
            frames: 32,
            first_frame: 0,
            monotonic_ns: None,
            discontinuity: false,
        };
        processor
            .process_external_cycle(cycle, &mut Io)
            .unwrap()
            .unwrap();
        let (device, _) = super::super::tests::synthetic_device(2, 48_000, 48_000, false);
        let set = DeviceSet(
            BTreeMap::from([("virtual/render".into(), Arc::clone(&device))]),
            BTreeMap::new(),
        );
        let unrelated = ProcessReport {
            input_starvations: 1,
            output_overflows: 1,
            discontinuities: 1,
            ..Default::default()
        };
        set.record_latency_cycle(&processor, Some(unrelated), 1);
        assert_eq!(device.native.latency.faults.load(Ordering::Acquire), 0);
        assert!(
            device
                .native
                .latency
                .compensation_ready
                .load(Ordering::Acquire)
        );
        let direct = super::super::direct::DirectPlan {
            master: Arc::clone(&device),
            owners: vec![Arc::clone(&device)],
            latency_bindings: vec![("virtual/render".into(), Arc::clone(&device))],
            bindings: vec![],
            bridged: set,
            clock_id: 1,
        };
        direct.record_latency(&processor, unrelated, Some(IoTiming::default()), cycle);
        assert_eq!(device.native.latency.faults.load(Ordering::Acquire), 0);
    }

    /// The device worker replaces its converter and publishes the new epoch after the
    /// reporter read the taps. Taken as sampled, the stale cumulative taps became the new
    /// epoch's baseline and the next window's wrapped difference read 1024 frames as ~1.
    #[test]
    fn a_reconfiguration_between_tap_and_context_reads_is_neither_window_nor_baseline() {
        let (device, _context) = super::super::tests::synthetic_device(2, 48_000, 48_000, true);
        let converter = |frames| ResidenceSnapshot {
            sum: 1_024 * frames,
            weight: frames,
            invalid: 0,
        };
        let publish = |frames| device.native.latency.src_input.publish(converter(frames));
        publish(1_000_000);
        let baseline = observe(&device, false, read_taps);
        assert!(baseline.sampled);
        let torn = observe(&device, false, |device, direct| {
            let stale = read_taps(device, direct);
            device.native.latency.src_input.publish(converter(4_800));
            device.epoch.fetch_add(1, Ordering::Release);
            stale
        });
        assert!(!torn.sampled, "a context changed during the tap read");
        assert_ne!(torn.context(), baseline.context());
        publish(9_600);
        let next = observe(&device, false, read_taps);
        assert!(next.sampled);
        assert_eq!(next.instability(torn), Some("taps not sampled"));
        publish(14_400);
        let later = observe(&device, false, read_taps);
        assert_eq!(later.context(), next.context());
        assert_eq!(later.src_input.mean_since(next.src_input), Some(1_024.0));
    }

    /// A window over `frames` FIFO frames that each waited `ticks`.
    pub(crate) fn passage(ticks: u64, frames: u64) -> PassageSnapshot {
        let edge = |area| EdgeSnapshot {
            position: frames,
            area,
            latest: 0,
            discarded: 0,
        };
        PassageSnapshot {
            producer: edge(0),
            consumer: edge(ticks * frames),
        }
    }

    #[test]
    fn native_scalar_window_mean_scales_only_src_residence_domains() {
        let mut old = observation(false);
        old.native_rate = 48_000;
        let mut next = old;
        let sample = |sum| ResidenceSnapshot {
            sum,
            weight: 1,
            invalid: 0,
        };
        next.native.acquisition = sample(0);
        next.native.presentation = sample(0);
        // FIFO passages are host time already: 250 ms and 1 ms at 2 ns per tick.
        next.native.native_queue = passage(125_000_000, 32);
        next.native.graph_queue = passage(500_000, 32);
        next.src_input = sample(0);
        next.src_output = sample(0);
        assert_eq!(next.delay_ns(old, true, 2.0), Some(251_000_000.0));
        next.native.input_scalar = ResidenceSnapshot {
            sum: 2_002_000_000,
            weight: 2,
            invalid: 0,
        };
        next.native.output_scalar = sample(1_002_000_000);
        // A measured scalar no longer rescales host-timed FIFO passages.
        assert_eq!(next.delay_ns(old, true, 2.0), Some(251_000_000.0));
        next.src_input = sample(96);
        next.src_output = sample(96);
        next.src_delay = 48;
        let input = 251_000_000.0 + 1_001_000.0 + 1_001_000.0 + 1_000_000.0;
        let output = 251_000_000.0 + 1_002_000.0 + 1_002_000.0 + 1_002_000.0;
        assert!((next.delay_ns(old, true, 2.0).unwrap() - input).abs() < 0.001);
        assert!((next.delay_ns(old, false, 2.0).unwrap() - output).abs() < 0.001);
    }

    #[test]
    fn residence_uses_each_converter_output_rate_and_direct_has_no_queues() {
        let old = observation(false);
        let mut next = old;
        let sample = |sum| ResidenceSnapshot {
            sum,
            weight: 1,
            invalid: 0,
        };
        next.native.acquisition = sample(100);
        next.native.presentation = sample(50);
        next.native.native_queue = passage(88, 4);
        next.native.graph_queue = passage(96, 4);
        next.src_input = sample(44);
        next.src_output = sample(48);
        next.src_delay = 10;
        let input = 100.0
            + 88.0
            + 96.0
            + 44.0 * 0.5e9 / 44_100.0
            + 48.0 * 0.5e9 / 48_000.0
            + 10.0 * 1e9 / 48_000.0;
        assert!((next.delay_ns(old, true, 1.0).unwrap() - input).abs() < 0.001);
        let output = 50.0
            + 88.0
            + 96.0
            + 44.0 * 0.5e9 / 48_000.0
            + 48.0 * 0.5e9 / 44_100.0
            + 10.0 * 1e9 / 44_100.0;
        assert!((next.delay_ns(old, false, 1.0).unwrap() - output).abs() < 0.001);
        assert_eq!(next.instability(old), None);
        next.native.graph_queue.consumer.discarded += 1;
        assert_eq!(next.instability(old), Some("graph queue residence invalid"));
        assert_eq!(next.delay_ns(old, true, 1.0), None);
        next.native.graph_queue.consumer.discarded -= 1;
        next.faults += 1;
        assert!(next.instability(old).is_some());
        next.direct = true;
        assert_eq!(next.delay_ns(old, true, 1.0), Some(100.0));
        assert_eq!(next.delay_ns(old, false, 1.0), Some(50.0));
        next.native.acquisition.invalid = 1;
        assert_eq!(next.delay_ns(old, true, 1.0), None);
        let values = BTreeMap::from([("source/capture".into(), old), ("sink/render".into(), next)]);
        assert!(binding(&values, "source", true).is_some());
        assert!(binding(&values, "sink", false).is_some());
        assert!(binding(&values, "source", false).is_none());
    }

    #[test]
    fn intentional_capture_seek_uses_valid_acquired_age_but_unknown_age_stays_unstable() {
        let old = observation(false);
        let mut next = old;
        next.native.graph_queue.consumer.discarded += 1;
        next.native.content_age = ResidenceSnapshot {
            sum: 512 * 1_000_000,
            weight: 512,
            invalid: 0,
        };
        assert_eq!(next.instability(old), None);
        assert_eq!(next.delay_ns(old, true, 1.0), Some(1_000_000.0));
        next.native.content_age.invalid += 1;
        assert_eq!(next.instability(old), Some("graph queue residence invalid"));
        assert_eq!(next.delay_ns(old, true, 1.0), None);
    }
}
