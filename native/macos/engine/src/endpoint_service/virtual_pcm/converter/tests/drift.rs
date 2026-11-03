//! A consumer that reads an independent-clock source by time, as a device-paced graph
//! does, while its device clock drifts from the native clock.
use super::*;
use mixrelay_macos_transport::source_registry::{
    GraphCredit, ReadMiss, content_latency::ReadTiming,
};

const RATE: u32 = 48_000;
/// Two minutes of native periods.
const PERIODS: u64 = 120 * RATE as u64 / 128;
/// Periods before the correction must have settled.
const SETTLE: u64 = 2 * RATE as u64 / 128;

/// Native frame `frame` is captured at host nanosecond `native_time(frame)`.
fn native_time(frame: u64) -> u64 {
    1_000_000_000 + frame * 1_000_000_000 / u64::from(RATE)
}

/// The producer side: a client queue, the pump's mixing and the sinc converter.
struct Producer {
    registry: SourceRegistry,
    transport: SourceTransport,
    converter: Converter,
    writer: ClientPcmWriter,
    queues: Vec<ClientPcmReader>,
    mixer: ClientPcmMixer,
    nanos: Arc<AtomicU64>,
    scratch: [f32; 256],
    input: [f32; 256],
    times: [u64; 128],
}

impl Producer {
    fn new() -> Self {
        let nanos = Arc::new(AtomicU64::new(0));
        let registry =
            crate::endpoint_service::virtual_pcm::test_storage::registry(Box::new(Clock {
                nanos: nanos.clone(),
            }));
        let mut request = SourceRequest {
            capacity_frames: 16_384,
            channels: 2,
            transport_version: VERSION,
            ..SourceRequest::default()
        };
        for (target, byte) in request.source_id.iter_mut().zip(b"virtual:fixture") {
            *target = std::ffi::c_char::from_ne_bytes([*byte]);
        }
        let mut transport = SourceTransport::create(&registry, &request).unwrap();
        // A device-paced graph is not this source's clock master: conversion is sinc.
        let _ = registry.source_time(source_token("virtual:fixture"), RATE, 128);
        let clock = transport.clock().unwrap();
        let converter = started(&mut transport, clock, (RATE, 2));
        let (writer, queues) = client_grants();
        Self {
            registry,
            transport,
            converter,
            writer,
            queues,
            mixer: ClientPcmMixer::default(),
            nanos,
            scratch: [0.0; 256],
            input: [0.0; 256],
            times: [0; 128],
        }
    }

    /// The client renders one native period and the pump converts it.
    fn period(&mut self, first: u64) {
        let pcm: [f32; 256] = std::array::from_fn(|index| {
            f32::from(u16::try_from((first + index as u64 / 2) % 30_000 + 1).unwrap()) / 32_768.0
        });
        assert!(self.writer.write(first, &pcm, 128));
        let playhead = first + 128;
        self.nanos
            .store(native_time(playhead) - native_time(0), Ordering::Relaxed);
        while let Some(window) = self.mixer.window(&self.queues, |_| Some(playhead)) {
            let frames = window.frames as usize;
            let input = &mut self.input[..frames * 2];
            ClientPcmMixer::mix(&self.queues, window, input, &mut self.scratch).unwrap();
            for (offset, time) in self.times[..frames].iter_mut().enumerate() {
                *time = native_time(window.first + offset as u64);
            }
            let clock = self.transport.clock().unwrap();
            self.converter.observe(&self.transport, clock).unwrap();
            let times = &self.times[..frames];
            self.converter
                .push(&mut self.transport, input, times)
                .unwrap();
            assert!(self.converter.flush(&mut self.transport).unwrap());
            self.mixer.commit(window);
            let quantum = u32::try_from(self.converter.output_quantum()).unwrap();
            self.transport.publish_measurements(128, quantum, 0);
        }
    }
}

/// A graph whose device clock runs `drift_ppm` slow, reading by time 40 ms behind the
/// newest PCM.
#[derive(Default)]
struct Consumer {
    period: f64,
    device: f64,
    cursor: Option<f64>,
    position: Option<u64>,
    misses: Vec<ReadMiss>,
    skipped: u64,
}

impl Consumer {
    #[expect(clippy::cast_precision_loss, reason = "host nanoseconds below 2^52")]
    fn new() -> Self {
        Self {
            device: native_time(0) as f64,
            ..Self::default()
        }
    }

    /// The device clock now runs `drift_ppm` slow.
    fn drift(&mut self, drift_ppm: f64) {
        self.period = 1e9 / f64::from(RATE) * (1.0 + drift_ppm * 1e-6);
    }

    /// Every device block the device clock completed by the native playhead.
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "host nanoseconds below 2^52"
    )]
    fn run(&mut self, producer: &Producer, playhead: u64, jump: f64) {
        let token = source_token("virtual:fixture");
        while self.device + 128.0 * self.period <= native_time(playhead) as f64 {
            self.device += 128.0 * self.period;
            let Some(mut requested) = self.cursor else {
                let credit = GraphCredit {
                    frames: 128,
                    available_frames: 128,
                    published: None,
                };
                // Join once 40 ms of PCM lies behind the safe frontier.
                self.cursor = producer
                    .registry
                    .source_time_for_credit(token, RATE, credit)
                    .filter(|frontier| *frontier > native_time(0) + 50_000_000)
                    .map(|frontier| frontier as f64 - 40e6);
                continue;
            };
            if jump != 0.0 {
                requested += jump;
                self.position = None;
            }
            self.cursor = Some(requested + 128.0 * self.period);
            let mut output = [0.0; 256];
            let timing = ReadTiming {
                rate: RATE,
                first_nanos: Some(requested.round() as u64),
                host_ticks: None,
            };
            match producer
                .registry
                .read_audio_timed(token, &mut output, 128, timing)
            {
                Ok(_) => {
                    let next = producer.transport.clock().unwrap().consumed_frames;
                    if let Some(previous) = self.position {
                        self.skipped += next.saturating_sub(previous + 128);
                    }
                    self.position = Some(next);
                }
                Err(miss) => {
                    self.misses.push(miss);
                    self.position = None;
                }
            }
        }
    }
}

/// Runs `periods` native periods against a device whose drift at each period
/// `drift(period)` gives, moving the cursor forward by `jump` nanoseconds halfway.
/// Returns the consumer, the producer, and the correction range once the drift has
/// been constant for two seconds.
fn hold(periods: u64, drift: impl Fn(u64) -> f64, jump: f64) -> (Consumer, Producer, (f64, f64)) {
    let mut producer = Producer::new();
    let mut consumer = Consumer::new();
    let mut settled = (f64::MAX, f64::MIN);
    let mut changed = 0;
    for period in 0..periods {
        let ppm = drift(period);
        if period == 0 || (ppm - drift(period - 1)).abs() > 50.0 {
            changed = period;
        }
        consumer.drift(ppm);
        producer.period(period * 128);
        let jump = if period == periods / 2 { jump } else { 0.0 };
        consumer.run(&producer, period * 128 + 128, jump);
        if period >= changed + SETTLE {
            let correction = producer.converter.correction_ppm();
            settled = (settled.0.min(correction), settled.1.max(correction));
        }
    }
    (consumer, producer, settled)
}

/// Two minutes at a constant drift: no read may miss, at most the one frame the first
/// period measurement may cost is skipped, and the correction settles at the drift.
fn constant(drift_ppm: f64, jump: f64) {
    let (consumer, producer, settled) = hold(PERIODS, |_| drift_ppm, jump);
    assert_eq!(consumer.misses, [], "a time-addressed read missed");
    assert!(
        consumer.skipped <= 1,
        "{} frames skipped at {drift_ppm} ppm",
        consumer.skipped
    );
    // A slower device needs fewer converted frames per native frame.
    assert!(
        settled.0 >= -drift_ppm - 20.0 && settled.1 <= -drift_ppm + 20.0,
        "correction left {settled:?} ppm at {drift_ppm} ppm of drift"
    );
    let correction = producer.converter.correction_ppm();
    assert!(
        (correction + drift_ppm).abs() < 5.0,
        "ended at {correction} ppm"
    );
}

/// Ninety seconds whose drift changes from 0 to `to_ppm` from 30 s on, over `ramp`
/// periods (0 for a step). The correction follows within `skipped` frames and no miss.
fn change(to_ppm: f64, ramp: u64, skipped: u64) {
    let start = 30 * u64::from(RATE) / 128;
    #[expect(clippy::cast_precision_loss, reason = "period counts below 2^24")]
    let drift = move |period: u64| {
        if period < start {
            0.0
        } else if period >= start + ramp {
            to_ppm
        } else {
            to_ppm * (period - start) as f64 / ramp as f64
        }
    };
    let (consumer, producer, _) = hold(90 * u64::from(RATE) / 128, drift, 0.0);
    assert_eq!(consumer.misses, [], "a time-addressed read missed");
    assert!(
        consumer.skipped <= skipped,
        "{} frames skipped changing to {to_ppm} ppm over {ramp} periods",
        consumer.skipped
    );
    let correction = producer.converter.correction_ppm();
    assert!(
        (correction + to_ppm).abs() < 5.0,
        "ended at {correction} ppm"
    );
}

#[test]
fn a_time_addressed_consumer_holds_two_minutes_without_drift_through_a_cursor_jump() {
    constant(0.0, 20e6);
}

#[test]
fn a_time_addressed_consumer_follows_a_slow_device_for_two_minutes() {
    constant(200.0, 0.0);
}

#[test]
fn a_time_addressed_consumer_follows_a_fast_device_for_two_minutes() {
    constant(-200.0, 0.0);
}

#[test]
fn a_time_addressed_consumer_follows_a_step_in_device_drift() {
    change(200.0, 0, 2);
    change(-200.0, 0, 2);
}

#[test]
fn a_time_addressed_consumer_follows_a_ramp_in_device_drift() {
    let ramp = 30 * u64::from(RATE) / 128;
    change(200.0, ramp, 3);
    change(-200.0, ramp, 3);
}

/// A cursor jump smaller than a frame (a frontier adopted on a frame grid after a
/// discontinuity) is not a clock change: the correction stays at the drift.
#[test]
fn a_sub_frame_cursor_jump_is_not_taken_for_drift() {
    for drift_ppm in [0.0, 200.0, -200.0] {
        for jump in [15e3, 25e3] {
            let (consumer, _, settled) = hold(30 * u64::from(RATE) / 128, |_| drift_ppm, jump);
            assert_eq!(consumer.misses, [], "{jump} ns jump at {drift_ppm} ppm");
            assert!(
                settled.0 >= -drift_ppm - 100.0 && settled.1 <= -drift_ppm + 100.0,
                "correction left {settled:?} ppm after a {jump} ns jump at {drift_ppm} ppm"
            );
        }
    }
}

/// A device clock whose block periods jitter by up to 50 ppm around 200 ppm of drift.
#[test]
fn a_time_addressed_consumer_holds_through_rate_noise() {
    let noise = |period: u64| {
        let hash = period
            .wrapping_mul(6_364_136_223_846_793_005)
            .rotate_left(29);
        #[expect(clippy::cast_precision_loss, reason = "a 16-bit sample")]
        let unit = (hash & 0xffff) as f64 / 65_535.0;
        200.0 + 100.0 * unit - 50.0
    };
    let (consumer, producer, _) = hold(60 * u64::from(RATE) / 128, noise, 0.0);
    assert_eq!(consumer.misses, [], "a time-addressed read missed");
    assert!(consumer.skipped <= 1, "{} frames skipped", consumer.skipped);
    let correction = producer.converter.correction_ppm();
    assert!(
        (correction + 200.0).abs() < 20.0,
        "ended at {correction} ppm"
    );
}
