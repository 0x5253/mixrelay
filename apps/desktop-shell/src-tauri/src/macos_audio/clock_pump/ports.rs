//! What the pump needs from the tap, the shared transport, the playback lease and time.
//! The application source implements these over the native owners; tests use fakes.
use mixrelay_macos_process_tap::{TapReadGeneration, TapStats};
use mixrelay_macos_transport::application_source_transport::SourceClockSnapshot;
use mixrelay_realtime::Wake;
use mixrelay_time::{MonotonicClock, Sleeper};
use std::time::Duration;

/// The process tap the pump reads: the ring's only consumer plus its telemetry.
pub(in crate::macos_audio) trait CaptureTap {
    fn source_id(&self) -> &str;
    fn channels(&self) -> usize;
    fn health(&self) -> TapStats;
    /// Waits up to `timeout` for the tap's next callback; `None` when the tap has no
    /// notifier and the pump must poll.
    fn wait_for_callback(&self, timeout: Duration) -> Option<TapWake>;
    /// Notifier signals the tap callback could not deliver.
    fn failed_signals(&self) -> u64;
    /// Reads timestamped frames of the expected native generation.
    fn read_timed(
        &mut self,
        samples: &mut [f32],
        times: &mut [u64],
        generation: TapReadGeneration,
    ) -> usize;
    /// Reads frames without timestamps; used only to discard a stale backlog.
    fn read_interleaved(&mut self, samples: &mut [f32]) -> usize;
    /// Applies a pending native format change.
    fn refresh_format(&mut self) -> Result<FormatState, String>;
}

/// The engine-shared ring the pump publishes converted PCM and clock telemetry into.
pub(in crate::macos_audio) trait SourceSink {
    /// The consumer's latest clock observation; `None` while it is unavailable.
    fn clock(&mut self) -> Option<SourceClockSnapshot>;
    /// Starts a new epoch for this format word and native rate.
    fn reset(&mut self, format_word: u64, native_rate: u32) -> bool;
    /// Marks the native producer stopped so the engine grants it no clock credit.
    fn stop_native(&mut self);
    fn observe_capture(&mut self, health: &TapStats);
    fn publish_measurements(&mut self, producer: u32, converter: u32, drought: u32);
    /// Records non-finite samples; `false` when the epoch was replaced.
    fn record_source_faults(&mut self, epoch: u64, count: u64) -> bool;
    /// Commits converted PCM; `false` when the epoch was replaced.
    fn write(&mut self, samples: &[f32], times: &[u64], frames: usize) -> bool;
}

/// Suppression of the captured app's own playback, held while the pump runs.
pub(in crate::macos_audio) trait PlaybackLease {
    fn refresh(&self) -> Result<(), String>;
    fn stop(&self) -> Result<(), String>;
}

/// The implementations one pump runs with.
pub(in crate::macos_audio) trait PumpPorts {
    type Tap: CaptureTap;
    type Sink: SourceSink;
    type Lease: PlaybackLease;
    type Clock: MonotonicClock + Sleeper;
}

/// How a wait for a tap callback ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::macos_audio) enum TapWake {
    /// A callback published while the pump waited; how late it woke, in host ticks.
    Woken {
        late_ticks: u64,
    },
    /// The callback had published before the wait began: the pump was busy.
    Missed,
    TimedOut,
    /// The notifier failed and is not trusted again.
    Failed,
}

impl TapWake {
    /// Classifies a notifier wait that began at host time `waiting_since`.
    pub(in crate::macos_audio) fn after(
        wake: Wake,
        waiting_since: u64,
        now: impl FnOnce() -> u64,
    ) -> Self {
        match wake {
            Wake::Notified { published_at } if published_at >= waiting_since => Self::Woken {
                late_ticks: now().saturating_sub(published_at),
            },
            Wake::Notified { .. } => Self::Missed,
            Wake::TimedOut => Self::TimedOut,
            Wake::Failed(_) => Self::Failed,
        }
    }
}

/// Whether the tap's native format is settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::macos_audio) enum FormatState {
    Ready,
    /// The native side is busy; retry later without destroying the tap.
    Pending,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_callback_published_during_the_wait_measures_wake_latency() {
        let notified = |published_at| Wake::Notified { published_at };
        assert_eq!(
            TapWake::after(notified(1_000), 1_000, || 1_250),
            TapWake::Woken { late_ticks: 250 }
        );
        assert_eq!(
            TapWake::after(notified(999), 1_000, || panic!(
                "no latency for a missed wake"
            )),
            TapWake::Missed
        );
        assert_eq!(
            TapWake::after(Wake::TimedOut, 1_000, || 0),
            TapWake::TimedOut
        );
        assert_eq!(
            TapWake::after(Wake::Failed(-1), 1_000, || 0),
            TapWake::Failed
        );
    }
}
