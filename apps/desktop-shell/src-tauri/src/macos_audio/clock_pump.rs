//! Owner of process-tap SRC. Conversion runs in the realtime class, woken by the tap
//! callback; control work (format and lease refresh, replacing captures or converters)
//! steps out of it. Feedback comes from the engine's consumed shared-ring cursor, never
//! capture amplitude or the private staging FIFO.
//!
//! The pump reaches the tap, the shared transport, the playback lease and time only
//! through its ports, so its cadence and recovery run against fakes in tests.
mod epoch;
pub(super) mod ports;
mod timing;
mod waker;

#[cfg(test)]
mod tests;

use crate::macos_audio::CAPTURE_RING_CAPACITY_FRAMES;
use epoch::ConversionEpoch;
use mixrelay_desktop_core::capture::ownership::CaptureHandoff;
use mixrelay_macos_process_tap::TapStats;
use mixrelay_macos_transport::application_source_transport::SourceClockSnapshot;
use mixrelay_realtime::{RealtimeThread, Timebase, Transition, demote_current_thread};
use mixrelay_time::{Instant, MonotonicClock};
use ports::{CaptureTap, FormatState, PlaybackLease, PumpPorts, SourceSink};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use timing::{DEFAULT_PERIOD, budget, callback_period, checked_rate};
use waker::Waker;

/// Frames converted per pass, matching the engine's block.
const ENGINE_BLOCK_FRAMES: usize = 128;
/// Bounded wait for progress no tap callback reports, and the polling interval.
const PUMP_IDLE_INTERVAL: Duration = Duration::from_micros(250);
/// Control work (format and lease refresh, retiming, reports) runs at most this often.
const CONTROL_INTERVAL: Duration = Duration::from_millis(100);
/// The shared ring counts as full once less than one engine block remains free.
const BACKPRESSURE_FRAMES: usize = CAPTURE_RING_CAPACITY_FRAMES - ENGINE_BLOCK_FRAMES;

/// What the control thread hands a running pump: a stop request and replacements.
pub(super) struct PumpMailbox<P: PumpPorts> {
    pub(super) stop: AtomicBool,
    pub(super) capture: CaptureHandoff<P::Tap>,
    pub(super) playback: CaptureHandoff<Option<Arc<P::Lease>>>,
}

impl<P: PumpPorts> Default for PumpMailbox<P> {
    fn default() -> Self {
        Self {
            stop: AtomicBool::new(false),
            capture: CaptureHandoff::default(),
            playback: CaptureHandoff::default(),
        }
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

/// Validates a tap's native sample rate before a pump is built for it.
pub(super) fn native_rate(rate: f64) -> Result<u32, String> {
    checked_rate(rate).map_err(|error| error.to_string())
}

/// Interleaved samples with one host timestamp per frame, allocated before promotion.
struct PcmBlock {
    samples: Vec<f32>,
    times: Vec<u64>,
}

impl PcmBlock {
    fn zeroed(channels: usize) -> Result<Self, String> {
        Ok(Self {
            samples: mixrelay_audio_io::pcm_buffer::zeroed(ENGINE_BLOCK_FRAMES * channels)
                .map_err(|error| error.to_string())?,
            times: mixrelay_audio_io::pcm_buffer::timestamps(ENGINE_BLOCK_FRAMES)
                .map_err(|error| error.to_string())?,
        })
    }
}

/// The playback lease a pump holds; released when replaced or when the pump ends.
struct ActivePlayback<L: PlaybackLease>(Option<Arc<L>>);

impl<L: PlaybackLease> Drop for ActivePlayback<L> {
    fn drop(&mut self) {
        if let Some(playback) = &self.0
            && let Err(error) = playback.stop()
        {
            tracing::error!(%error, "scoped playback restoration failed; native owner retained for teardown");
        }
    }
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

/// Pumps the tap into the transport until the mailbox asks it to stop, in the realtime
/// class when the system grants it.
pub(super) fn run<P: PumpPorts>(
    capture: P::Tap,
    transport: P::Sink,
    channels: usize,
    mailbox: &PumpMailbox<P>,
    clock: P::Clock,
) -> Result<(), String> {
    // Everything the pump owns is allocated before promotion and freed after demotion.
    let mut pump = Pump::<P>::new(capture, transport, channels, clock)?;
    let (period, computation, constraint) = budget(DEFAULT_PERIOD);
    let realtime = RealtimeThread::promote(period, computation, constraint)
        .inspect_err(|error| tracing::warn!(%error, "capture pump runs at standard priority"))
        .ok();
    let result = pump.run(mailbox, realtime.as_ref());
    if realtime.is_some() {
        // Leave the realtime class before the pump's converters, taps and playback
        // leases are released below, and before formatting any error.
        if let Err(error) = demote_current_thread() {
            tracing::warn!(%error, "capture pump stops in the realtime class");
        }
    }
    result.map_err(|error| error.to_string())
}

struct Pump<P: PumpPorts> {
    capture: P::Tap,
    transport: P::Sink,
    clock: P::Clock,
    channels: usize,
    input: PcmBlock,
    output: PcmBlock,
    epoch: Option<ConversionEpoch>,
    playback: ActivePlayback<P::Lease>,
    waker: Waker,
    format_pending: bool,
    last_refresh: Instant,
}

impl<P: PumpPorts> Pump<P> {
    fn new(
        capture: P::Tap,
        transport: P::Sink,
        channels: usize,
        clock: P::Clock,
    ) -> Result<Self, String> {
        let last_refresh = clock.now();
        Ok(Self {
            capture,
            transport,
            clock,
            channels,
            input: PcmBlock::zeroed(channels)?,
            output: PcmBlock::zeroed(channels)?,
            epoch: None,
            playback: ActivePlayback(None),
            waker: Waker::new(Timebase::current(), DEFAULT_PERIOD),
            format_pending: false,
            last_refresh,
        })
    }

    fn run(
        &mut self,
        mailbox: &PumpMailbox<P>,
        realtime: Option<&RealtimeThread>,
    ) -> Result<(), PumpError> {
        while !mailbox.stop.load(Ordering::Acquire) {
            self.step(mailbox, realtime)?;
        }
        Ok(())
    }

    /// One loop iteration: adopt replacements, run due control work, then convert.
    fn step(
        &mut self,
        mailbox: &PumpMailbox<P>,
        realtime: Option<&RealtimeThread>,
    ) -> Result<(), PumpError> {
        self.adopt(mailbox, realtime);
        if self.clock.now().duration_since(self.last_refresh) >= CONTROL_INTERVAL {
            self.format_pending = off_realtime(realtime, || self.control(realtime))?;
            self.last_refresh = self.clock.now();
        }
        if self.format_pending {
            self.hold_for_format(realtime);
            return Ok(());
        }
        self.pass(realtime)
    }

    /// Takes a published replacement without waiting for its publisher; the old tap,
    /// converter and lease are released outside the realtime class.
    fn adopt(&mut self, mailbox: &PumpMailbox<P>, realtime: Option<&RealtimeThread>) {
        let playback = mailbox.playback.try_take();
        let capture = mailbox.capture.try_take();
        if playback.is_none() && capture.is_none() {
            return;
        }
        off_realtime(realtime, || {
            if let Some(replacement) = playback {
                self.playback = ActivePlayback(replacement);
            }
            if let Some(replacement) = capture {
                // This pump is the only ring reader; the old tap is released here,
                // never on its Core Audio callback.
                self.capture = replacement;
                self.epoch = None;
                self.format_pending = false;
                self.last_refresh = self.clock.now();
                self.waker.replaced();
            }
        });
    }

    /// Periodic control work: report, retime, drop a revoked playback lease and refresh
    /// the tap format. Returns whether the format is still pending.
    fn control(&mut self, realtime: Option<&RealtimeThread>) -> Result<bool, PumpError> {
        self.waker
            .report(&self.capture, self.transport.clock(), self.clock.now());
        self.retime(realtime);
        self.refresh_playback().map_err(PumpError::Control)?;
        let format = self.capture.refresh_format().map_err(PumpError::Control)?;
        Ok(format == FormatState::Pending)
    }

    /// A changed exact process/device lease revokes suppression; ordinary shared
    /// capture continues, without reacquiring mute.
    fn refresh_playback(&mut self) -> Result<(), String> {
        let Some(playback) = &self.playback.0 else {
            return Ok(());
        };
        if let Err(error) = playback.refresh() {
            playback.stop()?;
            tracing::warn!(%error, "scoped playback lease revoked; capture preserved");
            // Already stopped: clear the holder in place so its drop does not stop it again.
            self.playback.0 = None;
        }
        Ok(())
    }

    /// While the format settles, the engine gets no clock credit and the epoch ends.
    fn hold_for_format(&mut self, realtime: Option<&RealtimeThread>) {
        self.transport.stop_native();
        if self.epoch.is_some() {
            off_realtime(realtime, || {
                if let Some(previous) = self.epoch.take() {
                    self.transport.reset(previous.format, previous.native_rate);
                }
            });
        }
        self.waker.briefly(&self.capture, &self.clock);
    }

    /// One conversion step, or a bounded wait when there is nothing to convert.
    fn pass(&mut self, realtime: Option<&RealtimeThread>) -> Result<(), PumpError> {
        let Some(consumer) = self
            .transport
            .clock()
            .filter(|consumer| consumer.format_word != 0)
        else {
            self.waker.briefly(&self.capture, &self.clock);
            return Ok(());
        };
        let health = self.capture.health();
        if !health.format_ready {
            // Native invalidation is recoverable. Do not certify mixed counters
            // as clock credit or mistake a refresh's temporary stop for retirement.
            self.transport.stop_native();
            self.waker.briefly(&self.capture, &self.clock);
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
            .epoch
            .as_ref()
            .is_none_or(|epoch| epoch.needs_reset(consumer, &health, rate))
        {
            return off_realtime(realtime, || {
                self.start_epoch(consumer, &health, rate, realtime)
            });
        }
        self.convert(consumer, &health)
    }

    /// Converts one block into the transport and publishes the producer measurements.
    fn convert(
        &mut self,
        consumer: SourceClockSnapshot,
        health: &TapStats,
    ) -> Result<(), PumpError> {
        let Some(epoch) = self.epoch.as_mut() else {
            return Ok(());
        };
        if epoch.observe(consumer)? >= BACKPRESSURE_FRAMES {
            epoch.backpressured();
            // The engine has not consumed yet; its progress is not signaled here.
            self.waker.briefly(&self.capture, &self.clock);
            return Ok(());
        }
        let (producer, converter) = epoch.measure_input(health.captured_frames);
        self.transport.publish_measurements(producer, converter, 0);
        let transferred = epoch.transfer(
            &mut self.capture,
            &mut self.transport,
            &mut self.input,
            &mut self.output,
        )?;
        if transferred == 0 {
            epoch.idle();
            // Converted output can be empty while tap input is still readable (the
            // converter is priming); read on instead of waiting for another callback.
            let readable = self.capture.health().available_frames > 0 && epoch.accepts_input();
            if !readable {
                self.waker.for_tap(&self.capture, &self.clock);
            }
            return Ok(());
        }
        if !self.transport.write(
            &self.output.samples[..transferred * self.channels],
            &self.output.times[..transferred],
            transferred,
        ) {
            // A format change races the worker safely: no old-rate PCM is accepted;
            // the next pass observes the new token and reconstructs off realtime.
            return Ok(());
        }
        let (producer, drought) = epoch.published(transferred, self.clock.now());
        self.transport
            .publish_measurements(producer, converter, drought);
        Ok(())
    }

    /// Control work: build the epoch's converter (allocating) and release the previous
    /// one, then fit the realtime budget to the tap's callback cadence.
    fn start_epoch(
        &mut self,
        consumer: SourceClockSnapshot,
        health: &TapStats,
        rate: u32,
        realtime: Option<&RealtimeThread>,
    ) -> Result<(), PumpError> {
        let fresh = ConversionEpoch::new(self.channels, rate, consumer, health.dropped_frames)
            .map_err(PumpError::Control)?;
        self.epoch = fresh.start(
            &mut self.capture,
            &mut self.transport,
            &mut self.input.samples,
            health,
        );
        self.retime(realtime);
        Ok(())
    }

    /// Control work: fits the realtime budget and tap wait to the tap's measured
    /// callback cadence; applied when the thread re-enters the realtime class.
    fn retime(&mut self, realtime: Option<&RealtimeThread>) {
        let health = self.capture.health();
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
