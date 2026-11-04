//! Real native callbacks, SRC queues, graph admission and a shared acquisition cursor.
use super::*;
use crate::macos_devices::{
    MacDevices,
    callback_fixture::{CallbackTime, NativeOwner, publish},
};
use mixrelay_audio_io::{process::ClockDomain, session::ClockAuthority};
use mixrelay_realtime::Timebase;

mod application_start;
mod startup;

#[test]
fn native_credit_bursts_reach_the_capture_frontier_before_graph_slicing() {
    let time = Arc::new(mixrelay_time::ManualClock::default());
    let status = Arc::new(AdapterRuntimeStatus::new(adapter_profile(), time));
    let resources = MacDevices::new(status.clone(), 48_000, 128);
    let devices = publish(&resources, &[]);
    let nanos = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let registry = mixrelay_macos_transport::source_registry::SourceRegistry::new(
        Box::new(SchedulingClock {
            nanos: nanos.clone(),
        }),
        mixrelay_realtime::Timebase::current(),
    );
    let mut application = ApplicationInput::new(&registry);
    let mut processor = application_processor();
    let mut timeline = MacTimeline::new();
    let mut pacer = ApplicationPacer::default();
    let route = RouteClock {
        maximum_frames: 128,
        synchronized: true,
        independent_idle: false,
    };
    let mut block = 8;
    application.publish(block);
    let format = (48_000, 128, 16_384);
    assert_eq!(
        pacer.next(
            (source_token("app"), application_progress(&registry)),
            format
        ),
        Pace::Wait
    );
    let mut capture = Capture {
        io: timeline.take_external_io(&devices),
        pcm: Vec::with_capacity(40_960),
    };
    let measured = allocation_counter::measure(|| {
        for blocks in [16, 128, 16] {
            for _ in 0..blocks {
                block += 1;
                application.publish(block);
            }
            nanos.store(
                u64::from(block) * 128 * 1_000_000_000 / 48_000,
                std::sync::atomic::Ordering::Relaxed,
            );
            while let pace @ Pace::Block { .. } = pacer.next(
                (source_token("app"), application_progress(&registry)),
                format,
            ) {
                let (frames, discontinuity) = timeline.accept((pace, 3), &route, &status).unwrap();
                assert!(timeline.available_frames >= frames);
                capture.io = timeline.take_external_io(&devices);
                capture.io.sources = Some(&registry);
                process_native_cycle(
                    &mut processor,
                    &mut capture,
                    WorkerCycle {
                        timeline: &timeline,
                        frames,
                        discontinuity,
                    },
                )
                .unwrap();
                timeline.capture_alignment = std::mem::take(&mut capture.io.alignment);
            }
        }
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(capture.pcm.len(), 40_960);
    let complete = capture
        .pcm
        .chunks_exact(2)
        .filter(|frame| {
            frame[0].to_bits() == 0.0_f32.to_bits() && frame[1].to_bits() == 0.25_f32.to_bits()
        })
        .count();
    assert_eq!(
        complete, 20_480,
        "retained source PCM was replaced by silence during native-credit catch-up"
    );
}

fn application_progress(
    registry: &mixrelay_macos_transport::source_registry::SourceRegistry,
) -> ApplicationClockSnapshot {
    let progress = registry.pcm_progress(source_token("app")).unwrap();
    let clock = progress.clock;
    ApplicationClockSnapshot {
        epoch: clock.epoch,
        published_frames: progress.published_frames,
        native_rate: clock.native_rate,
        graph_rate: clock.graph_rate,
        running: clock.running,
        synchronized: clock.synchronized,
        capacity_frames: progress.capacity_frames,
    }
}

struct Capture<'a> {
    io: MacEngineExternalIo<'a>,
    pcm: Vec<f32>,
}
impl ExternalIo for Capture<'_> {
    fn source_epoch(&self, binding: &ExternalBinding) -> Option<u64> {
        self.io.source_epoch(binding)
    }
    fn begin_cycle(
        &mut self,
        cycle: &ProcessCycle,
        controls: mixrelay_render::ExternalControlState<'_>,
    ) -> mixrelay_render::CycleAdmission {
        self.io.begin_cycle(cycle, controls)
    }
    fn read_source(
        &mut self,
        binding: &ExternalBinding,
        output: &mut [f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        self.io.read_source(binding, output, cycle)
    }
    fn write_sink(
        &mut self,
        _: &ExternalBinding,
        samples: &[f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        self.pcm.extend_from_slice(samples);
        TransferOutcome::complete(cycle.frames)
    }
}

struct ApplicationInput {
    owner: *mut mixrelay_macos_transport::application_source_transport::SourceTransport,
}

struct SchedulingClock {
    nanos: Arc<std::sync::atomic::AtomicU64>,
}
impl mixrelay_time::MonotonicClock for SchedulingClock {
    fn now(&self) -> mixrelay_time::Instant {
        mixrelay_time::Instant::from_origin(Duration::from_nanos(
            self.nanos.load(std::sync::atomic::Ordering::Relaxed),
        ))
    }
}
impl ApplicationInput {
    fn new(registry: &mixrelay_macos_transport::source_registry::SourceRegistry) -> Self {
        Self::at_rate(registry, 48_000)
    }

    fn at_rate(
        registry: &mixrelay_macos_transport::source_registry::SourceRegistry,
        native_rate: u32,
    ) -> Self {
        use mixrelay_macos_transport::{application_source_transport::*, source_control::*};
        use std::os::fd::IntoRawFd;
        let mut request = SourceRequest {
            capacity_frames: 16384,
            channels: 2,
            transport_version: mixrelay_macos_transport::application_clock::VERSION,
            ..SourceRequest::default()
        };
        for (slot, byte) in request.source_id.iter_mut().zip(b"app") {
            *slot = std::ffi::c_char::from_ne_bytes([*byte]);
        }
        let attached = registry.attach(&request).unwrap();
        let mut attachment = SourceAttachment {
            descriptor: attached.descriptor.into_raw_fd(),
            mapped_size: attached.mapped_size,
            producer_lease: attached.producer_lease,
            source_epoch: attached.source_epoch,
        };
        // SAFETY: this fixture transfers its unique descriptor to the production producer.
        let owner = unsafe {
            mr_application_source_transport_attach(
                &raw mut attachment,
                16384,
                2,
                std::ptr::null_mut(),
            )
        };
        assert!(!owner.is_null());
        let mut empty = [0.0; 256];
        registry.read_audio(source_token("app"), &mut empty, 128, 48_000);
        let mut clock = SourceClockSnapshot::default();
        // SAFETY: owner is live and the snapshot is correctly sized, exclusive storage.
        unsafe {
            assert!(mr_application_source_transport_clock(owner, &raw mut clock));
            assert!(mr_application_source_transport_reset(
                owner,
                clock.format_word,
                native_rate
            ));
            mr_application_source_transport_publish_native(owner, 0, true);
        }
        Self { owner }
    }
    fn publish(&mut self, block: u32) {
        use mixrelay_macos_transport::application_source_transport::*;
        let frames: [f32; 256] =
            std::array::from_fn(|index| if index % 2 == 0 { 0.0 } else { 0.25 });
        let times: [u64; 128] = std::array::from_fn(|offset| {
            1_000_000_000 + (u64::from(block) * 128 + offset as u64 - 1024) * 1_000_000_000 / 48_000
        });
        // SAFETY: this is the sole producer and both prepared spans contain 128 frames.
        unsafe {
            assert!(mr_application_source_transport_write_timed(
                self.owner,
                frames.as_ptr(),
                times.as_ptr(),
                128
            ));
            mr_application_source_transport_publish_native(
                self.owner,
                u64::from(block + 1) * 128,
                true,
            );
        }
    }
}
impl Drop for ApplicationInput {
    fn drop(&mut self) {
        // SAFETY: all producer calls drained; the independently owned registry still lives.
        unsafe {
            mixrelay_macos_transport::application_source_transport::mr_application_source_transport_destroy(self.owner);
        }
    }
}

fn application_processor() -> Box<BlockProcessor> {
    let document = serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":1,"nodes":[
            {"id":"a","type":"audio_device_input","config":{"selector":{"match":"persistent_uid","uid":"a"},"outputChannels":2}},
            {"id":"app","type":"application_capture","config":{"selector":{"match":"bundle_id","bundle_id":"fixture","include_descendants":false},"stream":{"channels":2,"channelLayout":"stereo"},"sourcePlayback":"preserve"}},
            {"id":"out","type":"audio_device_output","config":{"selector":{"match":"persistent_uid","uid":"out"},"inputChannels":2}}
        ],"edges":[
            {"id":"a-out","source":{"nodeId":"a","portId":"audio"},"destination":{"nodeId":"out","portId":"audio"}},
            {"id":"app-out","source":{"nodeId":"app","portId":"audio"},"destination":{"nodeId":"out","portId":"audio"}}
        ]
    })).unwrap();
    create_initial_processor(
        &document,
        RenderFormat {
            sample_rate: 48_000,
            block_frames: 128,
        },
        Arc::new(PeakMeterBank::new(8, 128)),
    )
    .unwrap()
}

#[test]
fn an_application_with_a_later_ready_frontier_joins_a_running_native_input() {
    let time = Arc::new(mixrelay_time::ManualClock::default());
    let status = Arc::new(AdapterRuntimeStatus::new(adapter_profile(), time));
    let resources = MacDevices::new(status, 48_000, 128);
    let input = NativeOwner::new(2, 48_000, true);
    let output = NativeOwner::new(2, 48_000, false);
    let devices = publish(&resources, &[("a", &input), ("out", &output)]);
    devices.set_clock_master(None);
    let _input_bridge = input.serve();
    let _output_bridge = output.serve();
    let nanos = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let registry = mixrelay_macos_transport::source_registry::SourceRegistry::new(
        Box::new(SchedulingClock {
            nanos: Arc::clone(&nanos),
        }),
        mixrelay_realtime::Timebase::current(),
    );
    let mut application = ApplicationInput::new(&registry);
    let mut io = MacEngineExternalIo::new(&devices);
    io.sources = Some(&registry);
    let mut capture = Capture {
        io,
        pcm: Vec::with_capacity(128 * 256),
    };
    let mut processor = application_processor();
    let timebase = Timebase::current();
    for block in 0..128 {
        nanos.store(
            u64::from(block) * 128 * 1_000_000_000 / 48_000,
            std::sync::atomic::Ordering::Relaxed,
        );
        let host = timebase.ticks(Duration::from_nanos(
            1_000_000_000 + u64::from(block) * 128 * 1_000_000_000 / 48_000,
        ));
        let mut native_pcm: [f32; 256] =
            std::array::from_fn(|index| if index % 2 == 0 { 0.1 } else { 0.0 });
        input.callback(
            &mut native_pcm,
            CallbackTime {
                now: host,
                host,
                sample: f64::from(block * 128),
                rate_scalar: 1.0,
            },
        );
        if block >= 20 {
            application.publish(block);
        }
        tick_output(&output, timebase, block);
        let measured = allocation_counter::measure(|| {
            processor
                .process_external_cycle(cycle(block), &mut capture)
                .unwrap()
                .unwrap();
        });
        assert_eq!(measured.count_total, 0);
    }
    assert!(capture.io.align_inputs);
    for frame in capture.pcm[12 * 256..16 * 256].chunks_exact(2) {
        assert!(
            (frame[0] - 0.1).abs() < 1e-4,
            "healthy input blocked by late application: {frame:?}"
        );
        assert_eq!(frame[1].to_bits(), 0);
    }
    for frame in capture.pcm[capture.pcm.len() - 4096..].chunks_exact(2) {
        assert!(
            (frame[1] - 0.25).abs() < 1e-4,
            "late application remained silent: {frame:?}"
        );
        assert!(
            (frame[0] - 0.1).abs() < 1e-4,
            "healthy input did not recover: {frame:?}"
        );
    }
}

fn processor() -> Box<BlockProcessor> {
    let document = serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":1,"nodes":[
            {"id":"a","type":"audio_device_input","config":{"selector":{"match":"persistent_uid","uid":"a"},"outputChannels":2,"gainDb":0}},
            {"id":"b","type":"audio_device_input","config":{"selector":{"match":"persistent_uid","uid":"b"},"outputChannels":2,"gainDb":0}},
            {"id":"out","type":"audio_device_output","config":{"selector":{"match":"persistent_uid","uid":"out"},"inputChannels":2}}
        ],"edges":[
            {"id":"a-out","source":{"nodeId":"a","portId":"audio"},"destination":{"nodeId":"out","portId":"audio"}},
            {"id":"b-out","source":{"nodeId":"b","portId":"audio"},"destination":{"nodeId":"out","portId":"audio"}}
        ]
    })).unwrap();
    create_initial_processor(
        &document,
        RenderFormat {
            sample_rate: 48_000,
            block_frames: 128,
        },
        Arc::new(PeakMeterBank::new(8, 128)),
    )
    .unwrap()
}

struct Input {
    rate: f64,
    frames: u32,
    first_ns: f64,
    sample: u32,
    channel: usize,
    pcm: Vec<f32>,
}

#[test]
fn an_unconnected_input_keeps_its_meter_without_retiming_the_connected_route() {
    let resources = MacDevices::new(
        Arc::new(AdapterRuntimeStatus::new(
            adapter_profile(),
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        )),
        48_000,
        128,
    );
    let a = NativeOwner::new(2, 48_000, true);
    let b = NativeOwner::new(2, 48_000, true);
    let output = NativeOwner::new(2, 48_000, false);
    let devices = publish(&resources, &[("a", &a), ("b", &b), ("out", &output)]);
    let _a_bridge = a.serve();
    let _b_bridge = b.serve();
    let _output_bridge = output.serve();
    let meters = Arc::new(PeakMeterBank::new(8, 128));
    let mut processor = unconnected_processor(Arc::clone(&meters));
    let binding = processor
        .plan()
        .meter_bindings()
        .iter()
        .position(|binding| {
            binding.node_id().as_str() == "b" && binding.port_id().as_str() == "audio"
        })
        .unwrap();
    let mut capture = Capture {
        io: MacEngineExternalIo::new(&devices),
        pcm: Vec::with_capacity(128 * 256),
    };
    let timebase = Timebase::current();
    for block in 0..128 {
        for (owner, origin, value) in [(&a, 2_000_000_000, 0.25), (&b, 1_000_000_000, 0.5)] {
            let host = timebase.ticks(Duration::from_nanos(
                origin + u64::from(block) * 128 * 1_000_000_000 / 48_000,
            ));
            owner.callback(
                &mut [value; 256],
                CallbackTime {
                    now: host,
                    host,
                    sample: f64::from(block * 128),
                    rate_scalar: 1.0,
                },
            );
        }
        tick_output(&output, timebase, block);
        let measured = allocation_counter::measure(|| {
            processor
                .process_external_cycle(cycle(block), &mut capture)
                .unwrap()
                .unwrap();
        });
        assert_eq!(measured.count_total, 0);
        if block == 100 {
            // Drain startup sinc overshoot before checking the ongoing meter.
            drop(meters.take_peaks(1, binding, 2));
        }
    }
    for sample in &capture.pcm[capture.pcm.len() - 4096..] {
        assert!(
            (*sample - 0.25).abs() < 1e-4,
            "unconnected clock muted the route: {sample}"
        );
    }
    let peaks = meters.take_peaks(1, binding, 2);
    assert!(
        peaks.iter().all(|peak| (*peak - 0.5).abs() < 1e-4),
        "unconnected meter: {peaks:?}"
    );
}

fn unconnected_processor(meters: Arc<PeakMeterBank>) -> Box<BlockProcessor> {
    let mut document: mixrelay_audio_graph::GraphDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":1,"nodes":[
            {"id":"a","type":"audio_device_input","config":{"selector":{"match":"persistent_uid","uid":"a"},"outputChannels":2}},
            {"id":"b","type":"audio_device_input","config":{"selector":{"match":"persistent_uid","uid":"b"},"outputChannels":2}},
            {"id":"out","type":"audio_device_output","config":{"selector":{"match":"persistent_uid","uid":"out"},"inputChannels":2}}
        ],"edges":[
            {"id":"a-out","source":{"nodeId":"a","portId":"audio"},"destination":{"nodeId":"out","portId":"audio"}}
        ]
    })).unwrap();
    document.nodes.push(mixrelay_audio_graph::NodeSpec {
        id: "unused-snapshot".into(),
        label: None,
        control: mixrelay_audio_graph::NodeControl::default(),
        config: mixrelay_audio_graph::NodeConfig::DeviceBus { channels: 2 },
    });
    document.edges.push(mixrelay_audio_graph::EdgeSpec {
        id: "b-snapshot".into(),
        source: mixrelay_audio_graph::AudioEndpoint {
            node_id: "b".into(),
            port_id: "audio".into(),
        },
        destination: mixrelay_audio_graph::AudioEndpoint {
            node_id: "unused-snapshot".into(),
            port_id: "audio".into(),
        },
        channel_mapping: mixrelay_audio_graph::ChannelMapping::default(),
    });
    create_initial_processor(
        &document,
        RenderFormat {
            sample_rate: 48_000,
            block_frames: 128,
        },
        meters,
    )
    .unwrap()
}

impl Input {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded synthetic timestamps and PCM"
    )]
    fn until(&mut self, now: f64, owner: &NativeOwner, timebase: Timebase) {
        while self.first_ns + f64::from(self.sample + self.frames) * 1e9 / self.rate <= now {
            let start = self.first_ns + f64::from(self.sample) * 1e9 / self.rate;
            for (offset, frame) in self.pcm.chunks_exact_mut(2).enumerate() {
                let time = start + f64::from(u32::try_from(offset).unwrap()) * 1e9 / self.rate;
                frame.fill(0.0);
                let event_distance = (time - 20e6 + 50e6).rem_euclid(100e6) - 50e6;
                frame[self.channel] = (-(event_distance / 150_000.0).powi(2)).exp() as f32 * 0.25;
            }
            let host = |nanos: f64| timebase.ticks(Duration::from_nanos(nanos.round() as u64));
            owner.callback(
                &mut self.pcm,
                CallbackTime {
                    now: host(now),
                    host: host(start),
                    sample: f64::from(self.sample),
                    rate_scalar: 1.0,
                },
            );
            self.sample += self.frames;
        }
    }
}

#[test]
fn missing_input_coordinates_do_not_mute_the_healthy_route() {
    let time = Arc::new(mixrelay_time::ManualClock::default());
    let status = Arc::new(AdapterRuntimeStatus::new(adapter_profile(), time));
    let resources = MacDevices::new(status, 48_000, 128);
    let a = NativeOwner::new(2, 48_000, true);
    let b = NativeOwner::new(2, 48_000, true);
    let output = NativeOwner::new(2, 48_000, false);
    let devices = publish(&resources, &[("a", &a), ("b", &b), ("out", &output)]);
    devices.set_clock_master(None);
    let _a_bridge = a.serve();
    let _b_bridge = b.serve();
    let _output_bridge = output.serve();
    let mut capture = Capture {
        io: MacEngineExternalIo::new(&devices),
        pcm: Vec::with_capacity(128 * 256),
    };
    let mut processor = processor();
    let timebase = Timebase::current();
    let mut healthy = [0.0; 256];
    let mut unknown = [0.0; 256];
    for frame in healthy.chunks_exact_mut(2) {
        frame[0] = 0.25;
    }
    for frame in unknown.chunks_exact_mut(2) {
        frame[1] = 0.5;
    }
    for block in 0..128 {
        let now = timebase.ticks(Duration::from_nanos(
            1_000_000_000 + u64::from(block) * 128 * 1_000_000_000 / 48_000,
        ));
        a.callback(
            &mut healthy,
            CallbackTime {
                now,
                host: now,
                sample: f64::from(block * 128),
                rate_scalar: 1.0,
            },
        );
        b.callback(
            &mut unknown,
            CallbackTime {
                now,
                host: 0,
                sample: f64::from(block * 128),
                rate_scalar: 1.0,
            },
        );
        tick_output(&output, timebase, block);
        let measured = allocation_counter::measure(|| {
            processor
                .process_external_cycle(cycle(block), &mut capture)
                .unwrap()
                .unwrap();
        });
        assert_eq!(measured.count_total, 0);
    }
    assert!(
        capture.io.align_inputs,
        "fixture must retain both active inputs"
    );
    assert!(capture.pcm.len() >= 4096);
    for frame in capture.pcm[capture.pcm.len() - 4096..].chunks_exact(2) {
        assert!(
            (frame[0] - 0.25).abs() < 1e-4,
            "healthy route held in priming: {frame:?}"
        );
        assert_eq!(
            frame[1].to_bits(),
            0,
            "unknown PCM was assigned invented coordinates"
        );
    }
}

fn cycle(block: u32) -> ProcessCycle {
    ProcessCycle {
        clock: ClockDomain {
            authority: ClockAuthority::Device,
            id: 77,
            epoch: 1,
        },
        sample_rate: 48_000,
        frames: 128,
        first_frame: u64::from(block) * 128,
        monotonic_ns: None,
        discontinuity: false,
    }
}

#[test]
fn independent_inputs_with_drift_and_different_bursts_align_the_same_event() {
    let time = Arc::new(mixrelay_time::ManualClock::default());
    let status = Arc::new(AdapterRuntimeStatus::new(adapter_profile(), time));
    let resources = MacDevices::new(status, 48_000, 128);
    let a = NativeOwner::at_rates(2, 48_000, 44_100, true);
    let b = NativeOwner::at_rates(2, 48_000, 48_000, true);
    let output = NativeOwner::new(2, 48_000, false);
    let devices = publish(&resources, &[("a", &a), ("b", &b), ("out", &output)]);
    // Both native rates differ from the graph's pacing clock, so neither bypasses SRC.
    devices.set_clock_master(None);
    let _a_bridge = a.serve();
    let _b_bridge = b.serve();
    let _output_bridge = output.serve();
    let mut a_input = Input {
        rate: 44_100.0 * 1.0003,
        frames: 64,
        first_ns: 17.0 * 1e9 / 48_000.0,
        sample: 0,
        channel: 0,
        pcm: vec![0.0; 128],
    };
    let mut b_input = Input {
        rate: 48_000.0 * 0.9998,
        frames: 512,
        first_ns: 73.0 * 1e9 / 48_000.0,
        sample: 0,
        channel: 1,
        pcm: vec![0.0; 1024],
    };
    let mut capture = Capture {
        io: MacEngineExternalIo::new(&devices),
        pcm: Vec::with_capacity(2_000_000),
    };
    let mut processor = processor();
    let timebase = Timebase::current();
    for block in 0..7500_u32 {
        let now = f64::from(block + 1) * 128.0 * 1e9 / 48_000.0;
        a_input.until(now, &a, timebase);
        b_input.until(now, &b, timebase);
        tick_output(&output, timebase, block);
        let measured = allocation_counter::measure(|| {
            processor
                .process_external_cycle(cycle(block), &mut capture)
                .unwrap()
                .unwrap();
        });
        assert_eq!(measured.count_total, 0);
    }
    let left = events(&capture.pcm, 0);
    let right = events(&capture.pcm, 1);
    assert_eq!(
        left.len(),
        197,
        "missing aligned events: left={left:?} right={right:?}"
    );
    assert_eq!(left.len(), right.len(), "left={left:?} right={right:?}");
    for (left, right) in left.iter().zip(right) {
        assert!(
            left.abs_diff(right) <= 1,
            "event mismatch: {left} vs {right}"
        );
    }
}

fn events(pcm: &[f32], channel: usize) -> Vec<usize> {
    pcm.chunks_exact(2)
        .map(|frame| frame[channel])
        .enumerate()
        .collect::<Vec<_>>()
        .windows(3)
        .filter_map(|triplet| {
            (triplet[1].0 > 12_000
                && triplet[1].1 > 0.2
                && triplet[1].1 > triplet[0].1
                && triplet[1].1 >= triplet[2].1)
                .then_some(triplet[1].0)
        })
        .collect::<Vec<_>>()
}

fn tick_output(output: &NativeOwner, timebase: Timebase, block: u32) {
    output.callback(
        &mut [0.0; 256],
        CallbackTime {
            now: timebase.ticks(Duration::from_nanos(
                u64::from(block + 1) * 128 * 1_000_000_000 / 48_000,
            )),
            host: timebase.ticks(Duration::from_nanos(
                u64::from(block + 2) * 128 * 1_000_000_000 / 48_000,
            )),
            sample: f64::from(block * 128),
            rate_scalar: 1.0,
        },
    );
}
