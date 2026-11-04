//! Pump cadence, recovery, handoff and failure handling against in-memory ports and a
//! manually advanced clock.
use super::ports::TapWake;
use super::*;
use mixrelay_macos_process_tap::TapReadGeneration;
use mixrelay_time::ManualClock;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    sync::atomic::AtomicUsize,
};

/// Generation 1 at a 48 kHz graph rate.
const FORMAT_WORD: u64 = (1 << 32) | 0xbb80;

/// A stereo 48 kHz tap whose ring holds `health.available_frames`.
pub(super) struct FakeTap {
    pub(super) id: u32,
    pub(super) health: TapStats,
    pub(super) notifier: bool,
    /// Scripted wake outcomes; a wait times out once they run out.
    pub(super) wakes: RefCell<VecDeque<TapWake>>,
    pub(super) waits: Cell<usize>,
    pub(super) timeouts: RefCell<Vec<Duration>>,
    /// Scripted refresh outcomes; the format is ready once they run out.
    pub(super) refreshes: VecDeque<Result<FormatState, String>>,
    pub(super) refresh_count: usize,
    /// `(format, clock_breaks)` of each timed read.
    pub(super) generations: Vec<(u64, u64)>,
    pub(super) timed_frames: usize,
    pub(super) discarded_frames: usize,
    next_time: u64,
}

impl Default for FakeTap {
    fn default() -> Self {
        Self {
            id: 0,
            health: TapStats {
                channels: 2,
                sample_rate: 48_000.0,
                running: true,
                format_ready: true,
                format_generation: 1,
                ..TapStats::default()
            },
            notifier: true,
            wakes: RefCell::default(),
            waits: Cell::new(0),
            timeouts: RefCell::default(),
            refreshes: VecDeque::new(),
            refresh_count: 0,
            generations: Vec::new(),
            timed_frames: 0,
            discarded_frames: 0,
            next_time: 0,
        }
    }
}

impl FakeTap {
    /// A tap without a notifier, so every wait sleeps on the pump's clock.
    pub(super) fn polled() -> Self {
        Self {
            notifier: false,
            ..Self::default()
        }
    }

    /// Native callbacks deliver `frames` more frames into the ring.
    fn deliver(&mut self, frames: usize) {
        self.health.callback_count += 1;
        self.health.captured_frames += u64::try_from(frames).unwrap();
        self.health.available_frames += frames;
    }

    fn take(&mut self, wanted: usize) -> usize {
        let frames = self.health.available_frames.min(wanted);
        self.health.available_frames -= frames;
        frames
    }
}

impl CaptureTap for FakeTap {
    fn source_id(&self) -> &'static str {
        "fake-source"
    }

    fn channels(&self) -> usize {
        usize::try_from(self.health.channels).unwrap()
    }

    fn health(&self) -> TapStats {
        self.health
    }

    fn wait_for_callback(&self, timeout: Duration) -> Option<TapWake> {
        if !self.notifier {
            return None;
        }
        self.waits.set(self.waits.get() + 1);
        self.timeouts.borrow_mut().push(timeout);
        Some(
            self.wakes
                .borrow_mut()
                .pop_front()
                .unwrap_or(TapWake::TimedOut),
        )
    }

    fn failed_signals(&self) -> u64 {
        0
    }

    fn read_timed(
        &mut self,
        samples: &mut [f32],
        times: &mut [u64],
        generation: TapReadGeneration,
    ) -> usize {
        self.generations
            .push((generation.format, generation.clock_breaks));
        let frames = self.take(times.len());
        samples[..frames * self.channels()].fill(0.25);
        for time in &mut times[..frames] {
            self.next_time += 20_833;
            *time = self.next_time;
        }
        self.timed_frames += frames;
        frames
    }

    fn read_interleaved(&mut self, samples: &mut [f32]) -> usize {
        let frames = self.take(samples.len() / self.channels());
        self.discarded_frames += frames;
        frames
    }

    fn refresh_format(&mut self) -> Result<FormatState, String> {
        self.refresh_count += 1;
        self.refreshes.pop_front().unwrap_or(Ok(FormatState::Ready))
    }
}

/// A shared ring whose engine consumer runs at 48 kHz and accepts every epoch and write
/// unless told otherwise.
struct FakeSink {
    consumer: Option<SourceClockSnapshot>,
    accept_reset: bool,
    accept_writes: bool,
    resets: Vec<(u64, u32)>,
    native_stops: usize,
    /// `running` of each capture health the pump reported.
    observed_running: Vec<bool>,
    write_attempts: usize,
    written_frames: usize,
    /// `(producer, converter, drought)` of each publication.
    measurements: Vec<(u32, u32, u32)>,
}

impl Default for FakeSink {
    fn default() -> Self {
        Self {
            consumer: Some(SourceClockSnapshot {
                format_word: FORMAT_WORD,
                consumer_running: 1,
                target_backlog_frames: 512,
                ..SourceClockSnapshot::default()
            }),
            accept_reset: true,
            accept_writes: true,
            resets: Vec::new(),
            native_stops: 0,
            observed_running: Vec::new(),
            write_attempts: 0,
            written_frames: 0,
            measurements: Vec::new(),
        }
    }
}

impl SourceSink for FakeSink {
    fn clock(&mut self) -> Option<SourceClockSnapshot> {
        self.consumer
    }

    fn reset(&mut self, format_word: u64, native_rate: u32) -> bool {
        self.resets.push((format_word, native_rate));
        if let Some(consumer) = self.consumer.as_mut().filter(|_| self.accept_reset) {
            consumer.epoch += 1;
            consumer.produced_frames = 0;
            consumer.consumed_frames = 0;
        }
        self.accept_reset
    }

    fn stop_native(&mut self) {
        self.native_stops += 1;
    }

    fn observe_capture(&mut self, health: &TapStats) {
        self.observed_running.push(health.running);
    }

    fn publish_measurements(&mut self, producer: u32, converter: u32, drought: u32) {
        self.measurements.push((producer, converter, drought));
    }

    fn record_source_faults(&mut self, epoch: u64, _count: u64) -> bool {
        self.consumer
            .is_some_and(|consumer| consumer.epoch == epoch)
    }

    fn write(&mut self, _samples: &[f32], _times: &[u64], frames: usize) -> bool {
        self.write_attempts += 1;
        if self.accept_writes {
            self.written_frames += frames;
            if let Some(consumer) = self.consumer.as_mut() {
                consumer.produced_frames += u64::try_from(frames).unwrap();
            }
        }
        self.accept_writes
    }
}

#[derive(Default)]
struct FakeLease {
    revoked: AtomicBool,
    stops: AtomicUsize,
}

impl FakeLease {
    fn stops(&self) -> usize {
        self.stops.load(Ordering::Relaxed)
    }
}

impl PlaybackLease for FakeLease {
    fn refresh(&self) -> Result<(), String> {
        if self.revoked.load(Ordering::Relaxed) {
            Err("lease revoked".into())
        } else {
            Ok(())
        }
    }

    fn stop(&self) -> Result<(), String> {
        self.stops.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

enum FakePorts {}

impl PumpPorts for FakePorts {
    type Tap = FakeTap;
    type Sink = FakeSink;
    type Lease = FakeLease;
    type Clock = ManualClock;
}

struct Harness {
    pump: Pump<FakePorts>,
    mailbox: PumpMailbox<FakePorts>,
    clock: ManualClock,
}

impl Harness {
    fn new(tap: FakeTap, sink: FakeSink) -> Self {
        let clock = ManualClock::default();
        Self {
            pump: Pump::new(tap, sink, 2, clock.clone()).unwrap(),
            mailbox: PumpMailbox::default(),
            clock,
        }
    }

    fn step(&mut self) -> Result<(), PumpError> {
        self.pump.step(&self.mailbox, None)
    }

    /// Runs the first pass, which opens an epoch on the transport.
    fn started(tap: FakeTap) -> Self {
        let mut harness = Self::new(tap, FakeSink::default());
        harness.step().unwrap();
        assert!(harness.pump.epoch.is_some());
        harness
    }

    /// Passes until the transport has committed converted PCM.
    fn until_written(&mut self) {
        for _ in 0..16 {
            self.pump.capture.deliver(128);
            self.step().unwrap();
            if self.pump.transport.written_frames > 0 {
                return;
            }
        }
        panic!("the pump never committed converted PCM");
    }

    fn after_control_interval(&mut self) -> Result<(), PumpError> {
        self.clock.advance(CONTROL_INTERVAL);
        self.step()
    }
}

fn at(duration: Duration) -> Instant {
    Instant::from_origin(duration)
}

#[test]
fn control_work_is_paced_by_pump_time_not_by_passes() {
    let sink = FakeSink {
        consumer: None,
        ..FakeSink::default()
    };
    let mut harness = Harness::new(FakeTap::polled(), sink);
    while harness.clock.now() < at(CONTROL_INTERVAL) {
        harness.step().unwrap();
    }
    assert_eq!(
        harness.pump.capture.refresh_count, 0,
        "400 idle passes in the first interval ran no control work"
    );
    while harness.clock.now() <= at(Duration::from_secs(1)) {
        harness.step().unwrap();
    }
    assert_eq!(harness.pump.capture.refresh_count, 10);
}

#[test]
fn tap_waits_follow_the_measured_callback_cadence() {
    let mut tap = FakeTap::default();
    // 256 frames per callback at 48 kHz: 5.333 ms.
    tap.health.callback_count = 100;
    tap.health.captured_frames = 25_600;
    let mut harness = Harness::started(tap);
    harness.step().unwrap();
    assert_eq!(
        harness.pump.capture.timeouts.borrow().last(),
        Some(&(Duration::from_nanos(5_333_333) * 2)),
        "an empty tap is waited on for two measured callback periods"
    );
    harness.pump.capture.deliver(64);
    let waits = harness.pump.capture.waits.get();
    harness
        .pump
        .transport
        .consumer
        .as_mut()
        .unwrap()
        .produced_frames = u64::try_from(BACKPRESSURE_FRAMES).unwrap();
    harness.step().unwrap();
    assert_eq!(
        harness.pump.capture.timeouts.borrow().last(),
        Some(&PUMP_IDLE_INTERVAL),
        "a full shared ring waits briefly for the engine, not for the tap"
    );
    assert_eq!(harness.pump.capture.waits.get(), waits + 1);
    assert_eq!(
        harness.pump.capture.health.available_frames, 64,
        "the tap was not read"
    );
}

#[test]
fn converted_pcm_is_committed_and_measured_only_when_the_engine_accepts_it() {
    let mut harness = Harness::started(FakeTap::default());
    harness.until_written();
    let written = harness.pump.transport.written_frames;
    let (producer, _, _) = *harness.pump.transport.measurements.last().unwrap();
    assert!(
        usize::try_from(producer).unwrap() >= written,
        "the publication burst covers the write"
    );

    harness.pump.transport.accept_writes = false;
    let attempts = harness.pump.transport.write_attempts;
    let published = harness.pump.transport.measurements.len();
    for _ in 0..8 {
        harness.pump.capture.deliver(128);
        harness.step().unwrap();
    }
    assert!(harness.pump.transport.write_attempts > attempts);
    assert_eq!(harness.pump.transport.written_frames, written);
    assert_eq!(
        harness.pump.transport.measurements.len(),
        published + 8,
        "a rejected write publishes only the pass's pre-transfer measurement"
    );
}

#[test]
fn a_busy_format_withholds_clock_credit_until_the_tap_settles() {
    let mut harness = Harness::started(FakeTap::polled());
    harness
        .pump
        .capture
        .refreshes
        .push_back(Ok(FormatState::Pending));
    harness.after_control_interval().unwrap();
    assert!(harness.pump.format_pending);
    assert!(harness.pump.epoch.is_none());
    assert_eq!(
        harness.pump.transport.resets,
        [(FORMAT_WORD, 48_000); 2],
        "the epoch was closed on the transport"
    );

    let stops = harness.pump.transport.native_stops;
    for _ in 0..10 {
        harness.pump.capture.deliver(128);
        harness.step().unwrap();
    }
    assert_eq!(harness.pump.capture.timed_frames, 0, "no PCM while pending");
    assert_eq!(harness.pump.transport.native_stops, stops + 10);
    assert_eq!(harness.pump.transport.resets.len(), 2, "closed only once");

    harness.after_control_interval().unwrap();
    assert!(!harness.pump.format_pending);
    assert!(
        harness.pump.epoch.is_some(),
        "a settled format opens a new epoch"
    );
    assert_eq!(harness.pump.transport.resets.len(), 3);
    assert_eq!(
        harness.pump.capture.discarded_frames, 1_280,
        "the backlog captured while pending is discarded"
    );
}

#[test]
fn a_native_timeline_break_opens_a_new_epoch_without_its_stale_backlog() {
    let mut harness = Harness::started(FakeTap::default());
    harness.pump.capture.deliver(128);
    harness.step().unwrap();
    assert_eq!(harness.pump.capture.generations.last(), Some(&(1, 0)));

    harness.pump.capture.deliver(300);
    harness.pump.capture.health.clock_discontinuities = 1;
    harness.step().unwrap();
    assert_eq!(harness.pump.transport.resets.len(), 2);
    assert_eq!(harness.pump.capture.discarded_frames, 300);

    harness.pump.capture.deliver(128);
    harness.step().unwrap();
    assert_eq!(
        harness.pump.capture.generations.last(),
        Some(&(1, 1)),
        "reads expect the new timeline"
    );
}

#[test]
fn a_refused_epoch_is_retried_on_the_next_pass() {
    let sink = FakeSink {
        accept_reset: false,
        ..FakeSink::default()
    };
    let mut harness = Harness::new(FakeTap::default(), sink);
    harness.step().unwrap();
    assert!(harness.pump.epoch.is_none());
    harness.pump.transport.accept_reset = true;
    harness.step().unwrap();
    assert!(harness.pump.epoch.is_some());
    assert_eq!(harness.pump.transport.resets.len(), 2);
}

#[test]
fn an_invalidated_format_is_recoverable_but_a_stopped_tap_ends_the_pump() {
    let mut harness = Harness::started(FakeTap::default());
    harness.pump.capture.health.format_ready = false;
    harness.pump.capture.deliver(128);
    harness.step().unwrap();
    assert_eq!(harness.pump.transport.native_stops, 1);
    assert_eq!(harness.pump.capture.timed_frames, 0);

    harness.pump.capture.health.format_ready = true;
    harness.pump.capture.health.running = false;
    let error = harness.step().unwrap_err();
    assert_eq!(error.to_string(), "native process tap stopped");
    assert_eq!(
        harness.pump.transport.observed_running.last(),
        Some(&false),
        "the transport sees the stopped tap and withdraws its credit"
    );
    assert_eq!(harness.pump.transport.native_stops, 1);

    let mut harness = Harness::started(FakeTap::default());
    harness.pump.capture.health.sample_rate = 0.0;
    assert_eq!(
        harness.step().unwrap_err().to_string(),
        "unsupported application capture sample rate: 0"
    );
}

#[test]
fn a_failed_format_refresh_ends_the_pump() {
    let mut harness = Harness::started(FakeTap::default());
    harness
        .pump
        .capture
        .refreshes
        .push_back(Err("application format refresh failed: 4 (-50)".into()));
    let error = harness.after_control_interval().unwrap_err();
    assert_eq!(
        error.to_string(),
        "application format refresh failed: 4 (-50)"
    );
}

#[test]
fn a_replacement_capture_is_adopted_with_a_fresh_epoch_and_notifier() {
    let mut harness = Harness::started(FakeTap::default());
    harness
        .pump
        .capture
        .wakes
        .borrow_mut()
        .push_back(TapWake::Failed);
    harness.step().unwrap();
    let waits = harness.pump.capture.waits.get();
    harness.step().unwrap();
    assert_eq!(
        harness.pump.capture.waits.get(),
        waits,
        "a failed notifier is polled instead"
    );

    harness.clock.advance(CONTROL_INTERVAL / 2);
    let mut replacement = FakeTap {
        id: 1,
        ..FakeTap::default()
    };
    replacement.deliver(200);
    harness.mailbox.capture.publish(replacement);
    harness.step().unwrap();
    assert_eq!(harness.pump.capture.id, 1);
    assert_eq!(harness.pump.capture.discarded_frames, 200);
    assert_eq!(harness.pump.transport.resets.len(), 2);

    harness.step().unwrap();
    assert_eq!(
        harness.pump.capture.waits.get(),
        1,
        "the replacement's notifier is trusted"
    );
    harness.clock.advance(CONTROL_INTERVAL / 2);
    harness.step().unwrap();
    assert_eq!(
        harness.pump.capture.refresh_count, 0,
        "control work is timed from the replacement"
    );
}

#[test]
fn playback_leases_are_released_when_replaced_or_revoked() {
    let mut harness = Harness::started(FakeTap::default());
    let first = Arc::new(FakeLease::default());
    harness.mailbox.playback.publish(Some(Arc::clone(&first)));
    harness.step().unwrap();
    let second = Arc::new(FakeLease::default());
    harness.mailbox.playback.publish(Some(Arc::clone(&second)));
    harness.step().unwrap();
    assert_eq!(first.stops(), 1, "the replaced lease was released");
    assert_eq!(second.stops(), 0);

    second.revoked.store(true, Ordering::Relaxed);
    harness.after_control_interval().unwrap();
    assert!(harness.pump.playback.0.is_none());
    assert_eq!(second.stops(), 1, "the revoked lease was released once");
    harness.until_written();
    drop(harness);
    assert_eq!(second.stops(), 1, "the pump does not release it again");
}
