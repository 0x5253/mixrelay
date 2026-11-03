//! Application capture ring -> actual pump SRC -> IPC v5 -> graph -> endpoint.
use super::*;
use mixrelay_audio_io::clock_bridge::{
    AsyncClockBridge, BridgeConfig, ClockFeedback, ClockRelation, Discontinuity, FeedbackQueue,
};
use mixrelay_macos_transport::{
    application_source_transport::*,
    capture_ring::*,
    source_control::{SourceAttachment, SourceRequest},
    source_registry::SourceRegistry,
};
use std::{
    os::fd::IntoRawFd,
    sync::atomic::{AtomicU64, Ordering},
};

struct SchedulingClock {
    nanos: Arc<AtomicU64>,
}
impl mixrelay_time::MonotonicClock for SchedulingClock {
    fn now(&self) -> mixrelay_time::Instant {
        mixrelay_time::Instant::from_origin(Duration::from_nanos(
            self.nanos.load(Ordering::Relaxed),
        ))
    }
}

struct Pump {
    owner: *mut SourceTransport,
    ring: *mut AudioRing,
    bridge: AsyncClockBridge,
    input: Vec<f32>,
    input_times: Vec<u64>,
    output: Vec<f32>,
    output_times: Vec<u64>,
    ticks: u64,
}
impl Pump {
    fn new(registry: &SourceRegistry) -> Self {
        let mut request = SourceRequest {
            capacity_frames: 16384,
            channels: 2,
            transport_version: mixrelay_macos_transport::application_clock::VERSION,
            ..Default::default()
        };
        for (slot, byte) in request.source_id.iter_mut().zip(b"input") {
            *slot = std::ffi::c_char::from_ne_bytes([*byte]);
        }
        let attached = registry.attach(&request).unwrap();
        let mut attachment = SourceAttachment {
            descriptor: attached.descriptor.into_raw_fd(),
            mapped_size: attached.mapped_size,
            producer_lease: attached.producer_lease,
            source_epoch: attached.source_epoch,
        };
        // SAFETY: transfers this fixture's unique descriptor to the production facade.
        let owner = unsafe {
            mr_application_source_transport_attach(
                &raw mut attachment,
                16384,
                2,
                std::ptr::null_mut(),
            )
        };
        assert!(!owner.is_null());
        let ring = mr_audio_ring_create(16384, 2);
        assert!(!ring.is_null());
        registry.read_audio(
            crate::macos::source_token("input"),
            &mut [0.0; 64],
            32,
            RATE,
        );
        let mut pump = Self {
            owner,
            ring,
            bridge: Self::bridge(RATE),
            input: vec![0.0; 1024],
            input_times: vec![0; 512],
            output: vec![0.0; 1024],
            output_times: vec![0; 512],
            ticks: 0,
        };
        pump.reset(RATE);
        pump
    }
    fn bridge(rate: u32) -> AsyncClockBridge {
        AsyncClockBridge::new(BridgeConfig {
            channels: 2,
            input_rate: rate,
            output_rate: RATE,
            max_input_frames: 512,
            max_output_frames: 512,
            capacity_frames: 16384,
            priming_frames: 256,
            clock_relation: ClockRelation::Independent,
            feedback_queue: FeedbackQueue::Output,
            max_correction_ppm: 2000.0,
        })
        .unwrap()
    }
    fn clock(&self) -> SourceClockSnapshot {
        let mut clock = SourceClockSnapshot::default();
        // SAFETY: retained owner and exclusive correctly sized snapshot storage.
        assert!(unsafe { mr_application_source_transport_clock(self.owner, &raw mut clock) });
        clock
    }
    fn reset(&mut self, rate: u32) {
        let format = self.clock().format_word;
        // SAFETY: serialized sole producer, with the consumer's current format word.
        assert!(unsafe { mr_application_source_transport_reset(self.owner, format, rate) });
        self.bridge = Self::bridge(rate);
        self.bridge
            .reset(self.clock().epoch, Discontinuity::FormatChanged);
        self.ticks = self.clock().consumer_ticks;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded synthetic host times and callback sizes"
    )]
    fn callback(&mut self, clock: &Clock<'_>, offset: f64) {
        let frames = clock.frames();
        fill_source(&mut self.input[..frames * 2], clock.position);
        // SAFETY: sole ring producer; complete PCM extent and acquired host coordinates.
        assert_eq!(
            unsafe {
                write_timed_interleaved(
                    self.ring,
                    self.input.as_ptr(),
                    frames,
                    (offset + clock.first_ns()).round() as u64,
                    1e9 / clock.rate,
                )
            },
            frames
        );
        let snapshot = self.clock();
        let elapsed = snapshot.consumer_ticks.saturating_sub(self.ticks);
        self.ticks = snapshot.consumer_ticks;
        self.bridge
            .observe_clock(ClockFeedback {
                queue: FeedbackQueue::Output,
                epoch: snapshot.epoch,
                queued_frames: usize::try_from(
                    snapshot
                        .produced_frames
                        .saturating_sub(snapshot.consumed_frames),
                )
                .unwrap(),
                target_frames: snapshot.target_backlog_frames.max(1) as usize,
                elapsed_frames: usize::try_from(elapsed.min(u64::from(RATE))).unwrap(),
                clock_running: snapshot.consumer_running != 0
                    && snapshot.target_backlog_frames != 0
                    && elapsed > 0
                    && elapsed <= u64::from(RATE),
            })
            .unwrap();
        // SAFETY: sole ring consumer with prepared PCM and coordinate spans.
        let read = unsafe {
            mr_audio_ring_read_timed(
                self.ring,
                self.input.as_mut_ptr(),
                self.input_times.as_mut_ptr(),
                512,
            )
        };
        self.bridge
            .push_timed_interleaved(&self.input[..read * 2], &self.input_times[..read])
            .unwrap();
        // The pump drains all available converter output before the next tap callback.
        for _ in 0..16 {
            let report = self
                .bridge
                .pull_timed_interleaved(&mut self.output, &mut self.output_times)
                .unwrap();
            if report.transferred_frames == 0 {
                break;
            }
            // SAFETY: sole producer; both spans contain transferred_frames initialized frames.
            assert!(unsafe {
                mr_application_source_transport_write_timed(
                    self.owner,
                    self.output.as_ptr(),
                    self.output_times.as_ptr(),
                    report.transferred_frames as u32,
                )
            });
        }
        let burst = (frames as u64 * u64::from(RATE)).div_ceil(clock.rate.round() as u64) as u32;
        // SAFETY: retained sole producer, matching the production pump's native/burst telemetry.
        unsafe {
            mr_application_source_transport_publish_measurements(
                self.owner,
                burst,
                self.bridge.processing_quantum().1 as u32,
                0,
            );
            mr_application_source_transport_publish_native(
                self.owner,
                clock.position + frames as u64,
                true,
            );
        }
    }
}
impl Drop for Pump {
    fn drop(&mut self) {
        // SAFETY: this in-process fixture has drained both serialized roles.
        unsafe {
            mr_application_source_transport_destroy(self.owner);
            mr_audio_ring_destroy(self.ring);
        }
    }
}

#[test]
fn application_capture_reports_actual_impulse_delay_with_variable_callbacks() {
    assert_application(&[RATE]);
}

#[test]
fn application_capture_reports_actual_impulse_delay_after_src_rate_change() {
    assert_application(&[RATE, 44_100]);
}

fn assert_application(rates: &[u32]) {
    let nanos = Arc::new(AtomicU64::new(0));
    let registry = SourceRegistry::with_clock_and_storage(
        Box::new(SchedulingClock {
            nanos: Arc::clone(&nanos),
        }),
        mixrelay_realtime::Timebase::current(),
        storage,
    );
    let mut pump = Pump::new(&registry);
    let status = Arc::new(AdapterRuntimeStatus::new(
        crate::macos::adapter_profile(),
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    ));
    let devices = MacDevices::without_devices(status, RATE, 32);
    let sink = NativeOwner::new(CHANNELS, RATE, false);
    let active = publish(&devices, &[("virtual/render", &sink)]);
    let mut worker = Worker {
        sources: Some(&registry),
        processor: route_processor(true),
        pacers: NativePacers::default(),
        timeline: MacTimeline::new(),
        status: AdapterRuntimeStatus::new(
            crate::macos::adapter_profile(),
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        ),
    };
    let plan = Arc::clone(worker.processor.plan());
    worker.render(&active);
    let _served = sink.serve();
    let mut sink_clock = Clock {
        rate: f64::from(RATE),
        quanta: &[512],
        input: false,
        callbacks: 0,
        position: 0,
    };
    let mut measurements = Measurements::default();
    for (index, rate) in rates.iter().enumerate() {
        if index > 0 {
            pump.reset(*rate);
            assert_eq!(
                application_report(&devices, &registry, &mut measurements, &plan),
                None,
                "a new native rate/epoch must clear the old stable report before priming"
            );
        }
        measure_phase(
            (&devices, &active),
            &mut worker,
            (&registry, &nanos),
            (&mut pump, &sink, &mut sink_clock),
            (&mut measurements, *rate),
        );
    }
}

fn application_report(
    devices: &MacDevices,
    sources: &SourceRegistry,
    measurements: &mut Measurements,
    plan: &RenderPlan,
) -> Option<u32> {
    let id = plan.sink_bindings()[0]
        .virtual_endpoint_id()
        .unwrap()
        .clone();
    let mut observations = devices.latency_observations(plan.revision(), false);
    super::super::append_observations(plan, Some(sources), false, &mut observations);
    measurements.observe(
        plan,
        plan.revision(),
        observations,
        &BTreeMap::from([(id.clone(), RATE)]),
    )[&id]
}

#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "bounded synthetic host coordinates"
)]
fn measure_phase(
    (devices, active): (&MacDevices, &DeviceSet),
    worker: &mut Worker<'_>,
    (registry, nanos): (&SourceRegistry, &AtomicU64),
    (pump, sink, sink_clock): (&mut Pump, &NativeOwner, &mut Clock<'_>),
    (measurements, rate): (&mut Measurements, u32),
) {
    let origin = sink_clock.now_ns();
    let offset = origin - START_NS;
    let mut source = Clock {
        rate: f64::from(rate) * (1.0 + SOURCE_PPM * 1e-6),
        quanta: VARIABLE_SOURCE,
        input: true,
        callbacks: 0,
        position: 0,
    };
    let ticks_per_ns =
        mixrelay_realtime::Timebase::current().ticks(Duration::from_secs(1)) as f64 / 1e9;
    let (mut arrivals, mut published) = (Arrivals::default(), Vec::new());
    let mut presentation = [0.0; 1024];
    let mut window = origin + WINDOW_NS;
    while sink_clock.now_ns() < origin + SETTLE_NS + MEASURE_NS + WINDOW_NS {
        let capture = offset + source.now_ns() <= sink_clock.now_ns();
        let now = if capture {
            offset + source.now_ns()
        } else {
            sink_clock.now_ns()
        };
        nanos.store(now as u64, Ordering::Relaxed);
        if now >= window {
            let value =
                application_report(devices, registry, measurements, worker.processor.plan());
            assert!(
                value.is_some() || window < origin + SETTLE_NS,
                "unmeasured application window: {:?}; source: {:?}; arrivals: {}; pump: {:?}; alignment: {:?}",
                measurements
                    .endpoints
                    .values()
                    .map(|endpoint| &endpoint.diagnosis)
                    .collect::<Vec<_>>(),
                registry.content_latency(crate::macos::source_token("input")),
                arrivals.crossings.len(),
                pump.clock(),
                worker.timeline.capture_alignment
            );
            published.push((window, value));
            window += WINDOW_NS;
        }
        if capture {
            pump.callback(&source, offset);
            source.advance();
        } else {
            sink.callback(&mut presentation, sink_clock.time(ticks_per_ns));
            arrivals.observe(&presentation, sink_clock.first_ns());
            worker.render(active);
            sink_clock.advance();
        }
    }
    let span = origin + SETTLE_NS..origin + SETTLE_NS + MEASURE_NS;
    let reported = published
        .iter()
        .filter(|(end, _)| span.contains(&(end - WINDOW_NS)))
        .map(|(_, value)| f64::from(value.expect("stable application latency")))
        .collect::<Vec<_>>();
    let actual = arrivals
        .crossings
        .iter()
        .filter_map(|presented| {
            let index = ((presented - origin) * source.rate / 1e9 / IMPULSE_PERIOD as f64).floor();
            let captured = origin + index * IMPULSE_PERIOD as f64 * 1e9 / source.rate;
            span.contains(&captured)
                .then_some((presented - captured) * f64::from(RATE) / 1e9)
        })
        .collect::<Vec<_>>();
    let (reported, actual) = (Spread::of(&reported), Spread::of(&actual));
    eprintln!(
        "application {rate} -> {RATE}, variable callbacks: reported {reported}, actual {actual} frames"
    );
    assert!(
        (reported.mean - actual.mean).abs() <= f64::from(threshold(RATE)),
        "reported {reported}, actual {actual} frames"
    );
}

fn storage(bytes: usize) -> std::io::Result<std::os::fd::OwnedFd> {
    let directory = mixrelay_test_support::FixtureDir::new("app-route")?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(directory.path().join("pcm"))?;
    file.set_len(u64::try_from(bytes).unwrap())?;
    Ok(file.into())
}
