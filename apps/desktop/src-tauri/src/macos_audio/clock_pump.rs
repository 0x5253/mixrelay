//! Owner of process-tap SRC. Conversion runs in the realtime class, woken by the tap
//! callback; control work (format and lease refresh, replacing captures or converters)
//! steps out of it. Feedback comes from the engine's consumed shared-ring cursor, never
//! capture amplitude or the private staging FIFO.
use super::{
    ApplicationSourceTransport, CAPTURE_RING_CAPACITY_FRAMES, ENGINE_BLOCK_FRAMES,
    NativeCaptureReader, NativeClockSnapshot, PUMP_IDLE_INTERVAL,
};
use mixrelay_audio_io::clock_bridge::{
    AsyncClockBridge, BridgeConfig, ClockFeedback, ClockRelation, Discontinuity, FeedbackQueue,
};
use mixrelay_realtime::{
    LatencyHistogram, LatencySummary, RealtimeThread, Timebase, Transition, Wake,
    demote_current_thread, host_time,
};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Until the tap's cadence is known, assume a common 512-frame callback at 48 kHz.
const DEFAULT_PERIOD: Duration = Duration::from_micros(10_667);

/// Realtime budget for one tap callback's worth of conversion: computation within 40%
/// and completion within 80% of the callback period.
fn budget(period: Duration) -> (Duration, Duration, Duration) {
    (period, period * 2 / 5, period * 4 / 5)
}

/// The tap's average callback length so far, bounded to plausible native quanta.
fn callback_period(health: &super::CaptureProbeStats, rate: u32) -> Duration {
    let frames = health
        .captured_frames
        .checked_div(health.callback_count)
        .unwrap_or(512)
        .clamp(64, 4_096);
    Duration::from_nanos(frames * 1_000_000_000 / u64::from(rate.max(1)))
}

/// Runs control work outside the realtime class, so it never blocks or spends the
/// realtime budget on property queries, lease checks, allocation or native teardown.
fn off_realtime<R>(realtime: Option<&RealtimeThread>, work: impl FnOnce() -> R) -> R {
    let Some(realtime) = realtime else {
        return work();
    };
    let (result, transition) = realtime.outside(work);
    match transition {
        Ok(()) => {}
        Err(Transition::Demote(error)) => {
            tracing::warn!(%error, "capture pump ran control work in the realtime class");
        }
        Err(Transition::Promote(error)) => {
            tracing::warn!(%error, "capture pump could not re-enter the realtime class");
        }
    }
    result
}

/// Waits for tap callbacks. Measures how late the pump wakes after a callback it was
/// waiting for, and counts wakes it was too busy to wait for, timeouts and failures.
struct Waker {
    timebase: Timebase,
    period: Duration,
    wakes: LatencyHistogram,
    busy: u64,
    timeouts: u64,
    failures: u64,
    /// A failed notifier is not trusted again for this capture; poll instead.
    degraded: bool,
    reported: Option<Instant>,
    reported_failed_signals: u64,
}

impl Waker {
    fn new() -> Self {
        Self {
            timebase: Timebase::current(),
            period: DEFAULT_PERIOD,
            wakes: LatencyHistogram::default(),
            busy: 0,
            timeouts: 0,
            failures: 0,
            degraded: false,
            reported: None,
            reported_failed_signals: 0,
        }
    }

    /// Waits for the tap's next callback, at most two callback periods; polls when
    /// the capture has no usable notifier.
    fn for_tap(&mut self, capture: &NativeCaptureReader) {
        self.wait(capture, self.period * 2);
    }

    /// Short bounded wait for progress no notification reports (the engine consuming,
    /// a format settling); a tap callback may still end it early.
    fn briefly(&mut self, capture: &NativeCaptureReader) {
        self.wait(capture, PUMP_IDLE_INTERVAL);
    }

    fn wait(&mut self, capture: &NativeCaptureReader, timeout: Duration) {
        let Some(wakeup) = capture.session().wakeup().filter(|_| !self.degraded) else {
            std::thread::sleep(PUMP_IDLE_INTERVAL);
            return;
        };
        let waiting_since = host_time();
        match wakeup.wait(timeout) {
            Wake::Notified { published_at } if published_at >= waiting_since => {
                let late = host_time().saturating_sub(published_at);
                self.wakes.record(self.timebase.nanoseconds(late));
            }
            Wake::Notified { .. } => self.busy += 1,
            Wake::TimedOut => self.timeouts += 1,
            Wake::Failed(_) => {
                self.failures += 1;
                self.degraded = true;
            }
        }
    }

    /// A replacement capture brings its own notifier.
    fn replaced(&mut self) {
        self.degraded = false;
    }

    /// Control step: logs the window every ten seconds.
    fn report(&mut self, capture: &NativeCaptureReader, clock: Option<NativeClockSnapshot>) {
        let now = Instant::now();
        if self
            .reported
            .is_some_and(|at| now.duration_since(at) < Duration::from_secs(10))
        {
            return;
        }
        self.reported = Some(now);
        if let Some(clock) = clock {
            tracing::info!(
                source_id = %capture.session().resolved.source_id,
                epoch = clock.epoch,
                target_frames = clock.target_backlog_frames,
                consumer_burst_frames = clock.consumer_burst_frames,
                starvation_episodes = clock.starvation_episodes,
                starvation_frames = clock.starvation_frames,
                rebuffer_frames = clock.rebuffer_frames,
                overwrite_skips = clock.overwrite_skips,
                insufficient_headroom = clock.insufficient_headroom,
                "application consumer latency"
            );
        }
        let failed_signals = capture
            .session()
            .wakeup()
            .map_or(0, mixrelay_realtime::Notifier::failed_signals);
        let window = self.wakes.take();
        let (busy, timeouts, failures) = (
            std::mem::take(&mut self.busy),
            std::mem::take(&mut self.timeouts),
            std::mem::take(&mut self.failures),
        );
        let new_failed_signals =
            std::mem::replace(&mut self.reported_failed_signals, failed_signals) != failed_signals;
        if window.is_none() && busy == 0 && timeouts == 0 && failures == 0 && !new_failed_signals {
            return;
        }
        let window = window.unwrap_or(LatencySummary {
            count: 0,
            p50_us: 0,
            p99_us: 0,
            p999_us: 0,
            maximum_us: 0,
        });
        tracing::info!(
            wakes = window.count,
            p50_us = window.p50_us,
            p99_us = window.p99_us,
            p999_us = window.p999_us,
            max_us = window.maximum_us,
            busy,
            timeouts,
            failures,
            failed_signals,
            degraded = self.degraded,
            period_us = self.period.as_micros(),
            "capture pump wakes after tap callbacks"
        );
    }
}

/// Pump failures. Variants raised on the realtime path carry no heap data; they are
/// formatted only after the pump has left the realtime class.
#[derive(Debug)]
enum PumpError {
    UnsupportedRate(f64),
    TapStopped,
    Bridge(mixrelay_audio_io::clock_bridge::BridgeError),
    Count(std::num::TryFromIntError),
    /// From control work, which already ran outside the realtime class.
    Control(String),
}

impl std::fmt::Display for PumpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedRate(rate) => {
                write!(
                    formatter,
                    "unsupported application capture sample rate: {rate}"
                )
            }
            Self::TapStopped => formatter.write_str("native process tap stopped"),
            Self::Bridge(error) => error.fmt(formatter),
            Self::Count(error) => error.fmt(formatter),
            Self::Control(error) => formatter.write_str(error),
        }
    }
}

fn checked_rate(rate: f64) -> Result<u32, PumpError> {
    if !rate.is_finite() || !(8_000.0..=192_000.0).contains(&rate) || rate.fract().abs() > 0.001 {
        return Err(PumpError::UnsupportedRate(rate));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "finite integral validated sample rate"
    )]
    Ok(rate as u32)
}

pub(super) fn native_rate(rate: f64) -> Result<u32, String> {
    checked_rate(rate).map_err(|error| error.to_string())
}

/// Round native/coalesced callback bursts upward into graph frames.
fn graph_frames(native_frames: u64, native_rate: u32, graph_rate: u32) -> u32 {
    u32::try_from(
        u128::from(native_frames)
            .saturating_mul(u128::from(graph_rate))
            .div_ceil(u128::from(native_rate.max(1))),
    )
    .unwrap_or(u32::MAX)
}

struct PumpClock {
    bridge: AsyncClockBridge,
    format: u64,
    native_rate: u32,
    ticks: u64,
    epoch: u64,
    dropped_frames: u64,
    queued_input: usize,
    native_generation: u64,
    native_discontinuities: u64,
    clock_master: bool,
    measured_native_frames: u64,
    producer_burst: u32,
    publication_burst: u32,
    previous_publication: Option<Instant>,
}

impl PumpClock {
    fn needs_reset(
        &self,
        clock: NativeClockSnapshot,
        health: &super::CaptureProbeStats,
        rate: u32,
    ) -> bool {
        self.format != clock.format_word
            || self.native_rate != rate
            || self.dropped_frames != health.dropped_frames
            || self.native_generation != health.format_generation
            || self.native_discontinuities != health.clock_discontinuities
            || self.clock_master != (clock.clock_master != 0)
    }

    fn new(
        channels: usize,
        rate: u32,
        clock: NativeClockSnapshot,
        dropped_frames: u64,
    ) -> Result<Self, String> {
        let graph_rate =
            u32::try_from(clock.format_word & u64::from(u32::MAX)).map_err(|e| e.to_string())?;
        let bridge = AsyncClockBridge::new(BridgeConfig {
            channels,
            input_rate: rate,
            output_rate: graph_rate,
            max_input_frames: ENGINE_BLOCK_FRAMES,
            max_output_frames: ENGINE_BLOCK_FRAMES,
            capacity_frames: CAPTURE_RING_CAPACITY_FRAMES,
            priming_frames: 256,
            clock_relation: if clock.clock_master != 0 {
                ClockRelation::ProvenSameClock
            } else {
                ClockRelation::Independent
            },
            feedback_queue: FeedbackQueue::Output,
            max_correction_ppm: 2_000.0,
        })
        .map_err(|e| e.to_string())?;
        Ok(Self {
            bridge,
            format: clock.format_word,
            native_rate: rate,
            ticks: clock.consumer_ticks,
            epoch: 0,
            dropped_frames,
            queued_input: 0,
            native_generation: 0,
            native_discontinuities: 0,
            clock_master: clock.clock_master != 0,
            measured_native_frames: 0,
            producer_burst: 0,
            publication_burst: 0,
            previous_publication: None,
        })
    }

    fn observe(&mut self, clock: NativeClockSnapshot) -> Result<usize, PumpError> {
        let queued = clock.produced_frames.saturating_sub(clock.consumed_frames);
        let queued = usize::try_from(queued).map_err(PumpError::Count)?;
        let elapsed = clock.consumer_ticks.saturating_sub(self.ticks);
        self.ticks = clock.consumer_ticks;
        // A stopped consumer does not turn clock-running merely because its last
        // published flag was true. No position advance means no PI update.
        let elapsed = usize::try_from(elapsed).map_err(PumpError::Count)?;
        let output_rate =
            usize::try_from(self.format & u64::from(u32::MAX)).map_err(PumpError::Count)?;
        self.bridge
            .observe_clock(ClockFeedback {
                queue: FeedbackQueue::Output,
                epoch: self.epoch,
                queued_frames: queued,
                target_frames: clock.target_backlog_frames.max(1) as usize,
                elapsed_frames: elapsed.min(output_rate),
                clock_running: clock.consumer_running != 0
                    && clock.target_backlog_frames != 0
                    && elapsed > 0
                    && elapsed <= output_rate,
            })
            .map_err(PumpError::Bridge)?;
        Ok(queued)
    }

    fn start_epoch(
        mut self,
        capture: &mut NativeCaptureReader,
        transport: &mut ApplicationSourceTransport,
        input: &mut [f32],
        health: &super::CaptureProbeStats,
    ) -> Option<Self> {
        if !transport.reset(self.format, self.native_rate) {
            return None;
        }
        // Discard only the prefix present at the discontinuity, not an unbounded
        // drain while the capture callback continues producing new frames.
        let mut remaining = health.available_frames;
        while remaining > 0 {
            let frames = capture.read_interleaved(input);
            if frames == 0 {
                break;
            }
            remaining = remaining.saturating_sub(frames);
        }
        self.epoch = transport.clock()?.epoch;
        self.measured_native_frames = health.captured_frames;
        self.native_generation = health.format_generation;
        self.native_discontinuities = health.clock_discontinuities;
        self.bridge.reset(self.epoch, Discontinuity::FormatChanged);
        Some(self)
    }

    fn measure_input(&mut self, captured: u64) -> (u32, u32) {
        // The low format-word half carries the graph rate.
        let [r0, r1, r2, r3, ..] = self.format.to_le_bytes();
        let graph_rate = u32::from_le_bytes([r0, r1, r2, r3]);
        let native_burst = captured.saturating_sub(self.measured_native_frames);
        self.measured_native_frames = captured;
        self.producer_burst =
            self.producer_burst
                .max(graph_frames(native_burst, self.native_rate, graph_rate));
        // Published measurements saturate like the publication burst.
        let converter = u32::try_from(self.bridge.processing_quantum().1).unwrap_or(u32::MAX);
        (self.producer_burst, converter)
    }

    fn published(&mut self, transferred: usize, now: Instant) -> (u32, u32) {
        self.publication_burst = self
            .publication_burst
            .saturating_add(u32::try_from(transferred).unwrap_or(u32::MAX));
        let graph_rate = self.format & u64::from(u32::MAX);
        let drought = self.previous_publication.map_or(0, |previous| {
            let elapsed = now
                .duration_since(previous)
                .as_nanos()
                .saturating_mul(u128::from(graph_rate))
                .div_ceil(1_000_000_000);
            u32::try_from(elapsed)
                .unwrap_or(u32::MAX)
                .saturating_sub(self.producer_burst)
        });
        self.previous_publication = Some(now);
        (self.producer_burst.max(self.publication_burst), drought)
    }

    fn transfer(
        &mut self,
        capture: &mut NativeCaptureReader,
        transport: &mut ApplicationSourceTransport,
        input: &mut (&mut [f32], &mut [u64]),
        output: &mut (&mut [f32], &mut [u64]),
    ) -> Result<usize, PumpError> {
        let channels = usize::from(capture.session().resolved.stream.channels);
        let frames = if self.queued_input < 256 {
            capture.read_timed(
                input.0,
                input.1,
                (self.native_generation, self.native_discontinuities),
            )
        } else {
            0
        };
        let samples = &mut input.0[..frames * channels];
        let faults = mixrelay_audio_io::pcm::sanitize_samples(samples);
        if !transport.record_source_faults(self.epoch, faults) {
            return Ok(0);
        }
        if frames > 0 {
            self.bridge
                .push_timed_interleaved(samples, &input.1[..frames])
                .map_err(PumpError::Bridge)?;
        }
        let report = self
            .bridge
            .pull_timed_interleaved(output.0, output.1)
            .map_err(PumpError::Bridge)?;
        self.queued_input = report.queued_input_frames;
        Ok(report.transferred_frames)
    }
}

pub(super) fn run_with_handoff(
    capture: NativeCaptureReader,
    transport: ApplicationSourceTransport,
    channels: usize,
    control: super::PumpControl<'_>,
) -> Result<(), String> {
    // Everything the pump owns is allocated before promotion and freed after demotion.
    let mut pump = Pump {
        capture,
        transport,
        channels,
        input: mixrelay_audio_io::pcm_buffer::zeroed(ENGINE_BLOCK_FRAMES * channels)
            .map_err(|error| error.to_string())?,
        output: mixrelay_audio_io::pcm_buffer::zeroed(ENGINE_BLOCK_FRAMES * channels)
            .map_err(|error| error.to_string())?,
        input_times: mixrelay_audio_io::pcm_buffer::timestamps(ENGINE_BLOCK_FRAMES)
            .map_err(|error| error.to_string())?,
        output_times: mixrelay_audio_io::pcm_buffer::timestamps(ENGINE_BLOCK_FRAMES)
            .map_err(|error| error.to_string())?,
        state: None,
        scoped: super::ActivePlayback(None),
        waker: Waker::new(),
        format_pending: false,
        last_refresh: Instant::now(),
    };
    let (period, computation, constraint) = budget(DEFAULT_PERIOD);
    let realtime = RealtimeThread::promote(period, computation, constraint)
        .inspect_err(|error| tracing::warn!(%error, "capture pump runs at standard priority"))
        .ok();
    let result = pump.run(&control, realtime.as_ref());
    if realtime.is_some() {
        // Leave the realtime class before the pump's converters, taps and playback
        // leases are released below, and before formatting any error.
        if let Err(error) = demote_current_thread() {
            tracing::warn!(%error, "capture pump stops in the realtime class");
        }
    }
    result.map_err(|error| error.to_string())
}

struct Pump {
    capture: NativeCaptureReader,
    transport: ApplicationSourceTransport,
    channels: usize,
    input: Vec<f32>,
    output: Vec<f32>,
    input_times: Vec<u64>,
    output_times: Vec<u64>,
    state: Option<PumpClock>,
    scoped: super::ActivePlayback,
    waker: Waker,
    format_pending: bool,
    last_refresh: Instant,
}

impl Pump {
    fn run(
        &mut self,
        control: &super::PumpControl<'_>,
        realtime: Option<&RealtimeThread>,
    ) -> Result<(), PumpError> {
        while !control.stop.load(Ordering::Acquire) {
            self.adopt(control, realtime);
            if self.last_refresh.elapsed() >= Duration::from_millis(100) {
                self.format_pending = off_realtime(realtime, || {
                    self.waker.report(&self.capture, self.transport.clock());
                    self.retime(realtime);
                    refresh(&mut self.scoped, &mut self.capture).map_err(PumpError::Control)
                })?;
                self.last_refresh = Instant::now();
            }
            if self.format_pending {
                self.transport.stop_native();
                if self.state.is_some() {
                    off_realtime(realtime, || {
                        if let Some(previous) = self.state.take() {
                            self.transport.reset(previous.format, previous.native_rate);
                        }
                    });
                }
                self.waker.briefly(&self.capture);
                continue;
            }
            self.pass(realtime)?;
        }
        Ok(())
    }

    /// Takes a published replacement without waiting for its publisher; the old tap,
    /// converter and lease are released outside the realtime class.
    fn adopt(&mut self, control: &super::PumpControl<'_>, realtime: Option<&RealtimeThread>) {
        let playback = control.playback.try_take();
        let capture = control.capture.try_take();
        if playback.is_none() && capture.is_none() {
            return;
        }
        off_realtime(realtime, || {
            if let Some(replacement) = playback {
                self.scoped = super::ActivePlayback(replacement);
            }
            if let Some(replacement) = capture {
                // This pump is the only ring reader; the old tap is released here,
                // never on its Core Audio callback.
                self.capture = replacement;
                self.state = None;
                self.format_pending = false;
                self.last_refresh = Instant::now();
                self.waker.replaced();
            }
        });
    }

    /// One conversion step, or a bounded wait when there is nothing to convert.
    fn pass(&mut self, realtime: Option<&RealtimeThread>) -> Result<(), PumpError> {
        let Some(clock) = self
            .transport
            .clock()
            .filter(|clock| clock.format_word != 0)
        else {
            self.waker.briefly(&self.capture);
            return Ok(());
        };
        let health = self.capture.session().stats();
        if !health.format_ready {
            // Native invalidation is recoverable. Do not certify mixed counters
            // as clock credit or mistake a refresh's temporary stop for retirement.
            self.transport.stop_native();
            self.waker.briefly(&self.capture);
            return Ok(());
        }
        // Raw capture activity can stop credit. Only converted PCM committed to
        // the shared transport can advance it; drops reset the epoch below.
        self.transport.observe_capture(&health);
        let rate = checked_rate(health.sample_rate)?;
        if !health.running {
            return Err(PumpError::TapStopped);
        }
        if self
            .state
            .as_ref()
            .is_none_or(|state| state.needs_reset(clock, &health, rate))
        {
            return off_realtime(realtime, || {
                self.start_epoch(clock, &health, rate, realtime)
            });
        }
        let Some(state) = self.state.as_mut() else {
            return Ok(());
        };
        if state.observe(clock)? >= CAPTURE_RING_CAPACITY_FRAMES - ENGINE_BLOCK_FRAMES {
            // Backpressure is not producer jitter or a coalesced publication.
            state.publication_burst = 0;
            state.previous_publication = None;
            // The engine has not consumed yet; its progress is not signaled here.
            self.waker.briefly(&self.capture);
            return Ok(());
        }
        let (producer, converter) = state.measure_input(health.captured_frames);
        self.transport.publish_measurements(producer, converter, 0);
        let transferred = state.transfer(
            &mut self.capture,
            &mut self.transport,
            &mut (&mut self.input, &mut self.input_times),
            &mut (&mut self.output, &mut self.output_times),
        )?;
        if transferred > 0
            && !self.transport.write(
                &self.output[..transferred * self.channels],
                &self.output_times[..transferred],
                transferred,
            )
        {
            // A format change races the worker safely: no old-rate PCM is accepted;
            // the next pass observes the new token and reconstructs off realtime.
            return Ok(());
        }
        if transferred > 0 {
            let (producer, drought) = state.published(transferred, Instant::now());
            self.transport
                .publish_measurements(producer, converter, drought);
        }
        if transferred == 0 {
            state.publication_burst = 0;
            // Converted output can be empty while tap input is still readable (the
            // converter is priming); read on instead of waiting for another callback.
            let readable =
                self.capture.session().stats().available_frames > 0 && state.queued_input < 256;
            if !readable {
                self.waker.for_tap(&self.capture);
            }
        }
        Ok(())
    }

    /// Control work: build the epoch's converter (allocating) and release the previous
    /// one, then fit the realtime budget to the tap's callback cadence.
    fn start_epoch(
        &mut self,
        clock: NativeClockSnapshot,
        health: &super::CaptureProbeStats,
        rate: u32,
        realtime: Option<&RealtimeThread>,
    ) -> Result<(), PumpError> {
        let fresh = PumpClock::new(self.channels, rate, clock, health.dropped_frames)
            .map_err(PumpError::Control)?;
        self.state = fresh.start_epoch(
            &mut self.capture,
            &mut self.transport,
            &mut self.input,
            health,
        );
        self.retime(realtime);
        Ok(())
    }

    /// Control work: fits the realtime budget and tap wait to the tap's measured
    /// callback cadence; applied when the thread re-enters the realtime class.
    fn retime(&mut self, realtime: Option<&RealtimeThread>) {
        let health = self.capture.session().stats();
        if !health.format_ready {
            return;
        }
        let Ok(rate) = checked_rate(health.sample_rate) else {
            return;
        };
        let period = callback_period(&health, rate);
        self.waker.period = period;
        if let Some(realtime) = realtime {
            let (period, computation, constraint) = budget(period);
            realtime.retune(period, computation, constraint);
        }
    }
}

/// Periodic control work: drop a revoked playback lease and refresh the tap format.
/// Returns whether the format is still pending.
fn refresh(
    scoped: &mut super::ActivePlayback,
    capture: &mut NativeCaptureReader,
) -> Result<bool, String> {
    if let Some(playback) = &scoped.0 {
        // A changed exact process/device lease revokes suppression;
        // ordinary shared capture continues, without reacquiring mute.
        if let Err(error) = playback.refresh() {
            playback.stop()?;
            tracing::warn!(%error, "scoped playback lease revoked; capture preserved");
            *scoped = super::ActivePlayback(None);
        }
    }
    refresh_capture(capture).map(|ready| !ready)
}

fn refresh_capture(capture: &mut NativeCaptureReader) -> Result<bool, String> {
    // SAFETY: sole format-control/ring-read owner. RESOURCE_BUSY retains the
    // native context and requires retry, not destruction or silent raw playback.
    let error = unsafe { super::mr_process_tap_refresh_format(capture.session().native.as_ptr()) };
    match error.code {
        super::NATIVE_OK => Ok(true),
        super::NATIVE_RESOURCE_BUSY => Ok(false),
        _ => Err(format!(
            "application format refresh failed: {} ({})",
            error.code, error.os_status
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn measured_native_coalesced_and_converter_bursts_are_graph_frames() {
        assert_eq!(graph_frames(128, 44_100, 48_000), 140);
        assert_eq!(graph_frames(1024, 96_000, 44_100), 471);
        assert_eq!(graph_frames(u64::MAX, 8000, 192_000), u32::MAX);
        let snapshot = NativeClockSnapshot {
            format_word: (1_u64 << 32) | 0xbb80,
            ..NativeClockSnapshot::default()
        };
        let mut clock = PumpClock::new(2, 48_000, snapshot, 0).unwrap();
        let (producer, converter) = clock.measure_input(256);
        assert_eq!(producer, 256);
        assert!(converter >= 128);
        assert_eq!(clock.measure_input(1280).0, 1024);
        assert_eq!(clock.measure_input(1280).0, 1024);
        let now = Instant::now();
        assert_eq!(clock.published(128, now), (1024, 0));
        assert_eq!(clock.published(1024, now), (1152, 0));
        let (_, drought) = clock.published(128, now + Duration::from_millis(30));
        assert_eq!(drought, 416);
    }

    #[test]
    fn published_target_changes_feedback_without_pcm_reset_and_inactive_freezes() {
        for master in [0, 1] {
            let mut snapshot = NativeClockSnapshot {
                format_word: (1_u64 << 32) | 0xbb80,
                produced_frames: 768,
                target_backlog_frames: 256,
                clock_master: master,
                consumer_running: 1,
                ..NativeClockSnapshot::default()
            };
            let mut clock = PumpClock::new(2, 48_000, snapshot, 0).unwrap();
            clock.bridge.push_interleaved(&[0.5; 256]).unwrap();
            for _ in 0..100 {
                snapshot.consumer_ticks += 128;
                clock.observe(snapshot).unwrap();
            }
            let correction = clock.bridge.correction_ppm();
            snapshot.target_backlog_frames = 1536;
            snapshot.consumer_running = 0;
            for _ in 0..100 {
                snapshot.consumer_ticks += 128;
                clock.observe(snapshot).unwrap();
            }
            assert!((clock.bridge.correction_ppm() - correction).abs() < f64::EPSILON);
            snapshot.consumer_running = 1;
            for _ in 0..1000 {
                snapshot.consumer_ticks += 128;
                clock.observe(snapshot).unwrap();
            }
            if master == 0 {
                assert!(clock.bridge.correction_ppm() > 0.0);
            } else {
                assert!(clock.bridge.correction_ppm().abs() < f64::EPSILON);
            }
            assert_eq!(clock.bridge.queued_input_frames(), 128);
            let correction = clock.bridge.correction_ppm();
            snapshot.target_backlog_frames = 0; // stale/unknown consumer publication
            snapshot.consumer_ticks += 128;
            clock.observe(snapshot).unwrap();
            assert!((clock.bridge.correction_ppm() - correction).abs() < f64::EPSILON);
        }
        assert_eq!(
            std::mem::offset_of!(NativeClockSnapshot, target_backlog_frames),
            48
        );
        assert_eq!(
            std::mem::offset_of!(NativeClockSnapshot, starvation_episodes),
            56
        );
    }

    #[test]
    fn the_realtime_budget_follows_the_tap_callback_cadence() {
        let mut health = super::super::CaptureProbeStats {
            callback_count: 0,
            captured_frames: 0,
            dropped_frames: 0,
            available_frames: 0,
            channels: 2,
            sample_rate: 48_000.0,
            format_generation: 1,
            clock_discontinuities: 0,
            running: true,
            format_ready: true,
            sample_time: 0.0,
            host_time: 0,
            clock_valid: false,
        };
        // Before any callback: the 512-frame default.
        assert_eq!(
            callback_period(&health, 48_000),
            Duration::from_nanos(10_666_666)
        );
        health.callback_count = 100;
        health.captured_frames = 12_800;
        assert_eq!(
            callback_period(&health, 48_000),
            Duration::from_nanos(2_666_666)
        );
        // Implausible averages are bounded to native quanta.
        health.captured_frames = 100;
        assert_eq!(
            callback_period(&health, 48_000),
            Duration::from_nanos(1_333_333)
        );
        let (period, computation, constraint) = budget(Duration::from_millis(10));
        assert_eq!(
            (period, computation, constraint),
            (
                Duration::from_millis(10),
                Duration::from_millis(4),
                Duration::from_millis(8)
            )
        );
    }

    #[test]
    fn timestamp_break_resets_even_without_a_rate_change_or_ring_overflow() {
        let snapshot = NativeClockSnapshot {
            format_word: (1_u64 << 32) | 0xbb80,
            ..NativeClockSnapshot::default()
        };
        let clock = PumpClock::new(2, 48_000, snapshot, 0).unwrap();
        let mut health = super::super::CaptureProbeStats {
            callback_count: 1,
            captured_frames: 128,
            dropped_frames: 0,
            available_frames: 128,
            channels: 2,
            sample_rate: 48_000.0,
            running: true,
            format_ready: true,
            format_generation: 0,
            clock_discontinuities: 0,
            sample_time: 0.0,
            host_time: 100,
            clock_valid: true,
        };
        assert!(!clock.needs_reset(snapshot, &health, 48_000));
        health.host_time = 100_000; // observation time is not a rate-change signal
        assert!(!clock.needs_reset(snapshot, &health, 48_000));
        health.clock_discontinuities = 1;
        assert!(clock.needs_reset(snapshot, &health, 48_000));
    }
    #[test]
    fn native_rates_are_validated_without_assuming_48k() {
        assert_eq!(native_rate(44_100.0).unwrap(), 44_100);
        assert_eq!(native_rate(96_000.0).unwrap(), 96_000);
        for rate in [f64::NAN, f64::INFINITY, -1.0, 0.0, 48_000.5, 384_000.0] {
            assert!(native_rate(rate).is_err());
        }
    }

    #[test]
    fn only_engine_consumption_drives_feedback_and_master_never_corrects_itself() {
        for master in [0, 1] {
            let mut snapshot = NativeClockSnapshot {
                format_word: (1_u64 << 32) | 0xbb80,
                produced_frames: 1_024,
                consumer_running: 1,
                target_backlog_frames: 512,
                clock_master: master,
                ..NativeClockSnapshot::default()
            };
            let mut clock = PumpClock::new(2, 48_000, snapshot, 0).unwrap();
            for _ in 0..1_000 {
                clock.observe(snapshot).unwrap();
            }
            assert!(
                clock.bridge.correction_ppm().abs() < f64::EPSILON,
                "worker ticks are not clock progress"
            );
            for _ in 0..1_000 {
                snapshot.consumer_ticks += 128;
                clock.observe(snapshot).unwrap();
            }
            if master == 0 {
                assert!(clock.bridge.correction_ppm() < 0.0);
            } else {
                assert!(clock.bridge.correction_ppm().abs() < f64::EPSILON);
            }
        }
    }

    #[test]
    fn source_converter_transfers_rate_scaled_prefix_only() {
        for (native, graph) in [(44_100_u32, 48_000_u32), (96_000, 44_100), (48_000, 48_000)] {
            let snapshot = NativeClockSnapshot {
                format_word: (1_u64 << 32) | u64::from(graph),
                ..NativeClockSnapshot::default()
            };
            let mut clock = PumpClock::new(2, native, snapshot, 0).unwrap();
            let input = [0.25_f32; 256];
            let mut output = [0.0; 256];
            let mut valid = 0_usize;
            for _ in 0..1_000 {
                clock.bridge.push_interleaved(&input).unwrap();
                loop {
                    let report = clock.bridge.pull_interleaved(&mut output).unwrap();
                    valid += report.transferred_frames;
                    assert!(
                        output[report.transferred_frames * 2..]
                            .iter()
                            .all(|sample| *sample == 0.0)
                    );
                    if report.transferred_frames == 0 {
                        break;
                    }
                }
            }
            let expected = 128_000_u64 * u64::from(graph) / u64::from(native);
            let valid = u64::try_from(valid).unwrap();
            assert!(
                valid <= expected && expected - valid < 512,
                "{native} → {graph}: {valid}/{expected}"
            );
            println!(
                "app SRC {native} → {graph}: raw baseline 128000 frames, converted {valid}, ideal {expected} (unflushed sinc tail)"
            );
        }
    }
}
