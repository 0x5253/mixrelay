//! Installed evidence of source admission latency and of client PCM the pump lost.
//! The pump records host ticks on RT; every log is written on control.
use mixrelay_macos_transport::client_pcm::ClientPcmClock;
use mixrelay_realtime::Timebase;
use std::time::Duration;

/// A loss episode ends once no client frame has been lost for this long; each episode is
/// reported once, with its frames.
const LOSS_QUIET: Duration = Duration::from_secs(1);

/// Milestones since the previous admission was logged, in host ticks.
#[derive(Default)]
struct Admission {
    first_pcm: Option<u64>,
    native: Option<([u8; 16], u64, u64)>,
    first_drain: Option<u64>,
    format: Option<u64>,
    deferrals: u32,
    converter: Option<(u64, ClientPcmClock, u64, u64)>,
}

/// Client frames that never reached the source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Losses {
    /// Overwritten in a client queue before the pump drained them.
    pub(super) skipped: u64,
    /// Dropped from the full engine backlog.
    pub(super) dropped: u64,
    /// Retained but not converted when their native timeline stopped.
    pub(super) stopped: u64,
}
impl Losses {
    fn since(self, earlier: Self) -> Self {
        Self {
            skipped: self.skipped.saturating_sub(earlier.skipped),
            dropped: self.dropped.saturating_sub(earlier.dropped),
            stopped: self.stopped.saturating_sub(earlier.stopped),
        }
    }
    pub(super) fn log(self) {
        tracing::warn!(
            boundary = "hal_shared_pcm",
            stage = "source_retention",
            queue_skipped_frames = self.skipped,
            retention_dropped_frames = self.dropped,
            stopped_unpublished_frames = self.stopped,
            "virtual input lost client PCM"
        );
    }
}

pub(super) struct Report {
    prepared: u64,
    source_started: u64,
    source_finished: u64,
    granted: Option<u64>,
    admission: Admission,
    stopped: u64,
    /// Totals when the last episode was reported, and as last observed.
    reported: Losses,
    seen: Losses,
    changed_at: u64,
}

impl Report {
    /// `prepared` is the host time the endpoint's pump was created on control.
    pub(super) fn new(prepared: u64) -> Self {
        Self {
            prepared,
            source_started: prepared,
            source_finished: prepared,
            granted: None,
            admission: Admission::default(),
            stopped: 0,
            reported: Losses::default(),
            seen: Losses::default(),
            changed_at: 0,
        }
    }
    pub(super) fn preparation(&mut self, started: u64, finished: u64) {
        self.source_started = started;
        self.source_finished = finished;
    }
    pub(super) fn granted(&mut self, received: u64) {
        self.granted = Some(received);
    }
    pub(super) fn drained(&mut self, now: u64, native: ClientPcmClock) {
        let identity = (native.incarnation, native.epoch, native.anchor);
        if self.admission.native != Some(identity) {
            self.admission = Admission {
                native: Some(identity),
                ..Admission::default()
            };
        }
        self.admission.first_drain.get_or_insert(now);
    }
    pub(super) fn retained(&mut self, now: u64) {
        self.admission.first_pcm.get_or_insert(now);
    }
    pub(super) fn format_requested(&mut self, now: u64) {
        self.admission.format.get_or_insert(now);
    }
    pub(super) fn deferred(&mut self) {
        self.admission.deferrals = self.admission.deferrals.saturating_add(1);
    }
    pub(super) fn converter_ready(
        &mut self,
        now: u64,
        native: ClientPcmClock,
        graph_rate: u64,
        retained: u64,
    ) {
        self.admission.converter = Some((now, native, graph_rate, retained));
    }
    pub(super) fn stopped(&mut self, frames: u64) {
        self.stopped = self.stopped.saturating_add(frames);
    }

    /// Whether a ready converter has yet to publish its first PCM.
    pub(super) fn awaiting_publication(&self) -> bool {
        self.admission.converter.is_some()
    }

    /// Logs every admission step relative to native sample zero (the device's IO start),
    /// then times the next admission. Control only.
    pub(super) fn log_admission(&mut self, now: u64, timebase: Timebase, published: usize) {
        let admission = std::mem::take(&mut self.admission);
        let Some((ready, native, graph_rate, retained)) = admission.converter else {
            return;
        };
        let since = |at: u64| {
            let milliseconds =
                |ticks| i64::try_from(timebase.nanoseconds(ticks) / 1_000_000).unwrap_or(i64::MAX);
            if at >= native.anchor {
                milliseconds(at - native.anchor)
            } else {
                -milliseconds(native.anchor - at)
            }
        };
        tracing::info!(
            boundary = "hal_shared_pcm",
            stage = "source_admission",
            native_rate = native.rate,
            graph_rate,
            endpoint_ms = since(self.prepared),
            grants_ms = ?self.granted.map(since),
            source_prepare_started_ms = since(self.source_started),
            source_prepare_finished_ms = since(self.source_finished),
            backlog_prepared_ms = since(self.prepared),
            first_drain_ms = ?admission.first_drain.map(since),
            first_pcm_ms = ?admission.first_pcm.map(since),
            format_ms = ?admission.format.map(since),
            converter_ms = since(ready),
            published_ms = since(now),
            epoch_deferrals = admission.deferrals,
            retained_frames = retained,
            published_frames = published,
            "virtual input source admitted"
        );
    }

    /// The frames of a loss episode once it has ended, reported once per episode.
    pub(super) fn due_losses(
        &mut self,
        now: u64,
        timebase: Timebase,
        (skipped, dropped): (u64, u64),
    ) -> Option<Losses> {
        let losses = Losses {
            skipped,
            dropped,
            stopped: self.stopped,
        };
        if losses != self.seen {
            self.seen = losses;
            self.changed_at = now;
            return None;
        }
        if losses == self.reported
            || now.saturating_sub(self.changed_at) < timebase.ticks(LOSS_QUIET)
        {
            return None;
        }
        let episode = losses.since(self.reported);
        self.reported = losses;
        Some(episode)
    }

    /// Remaining quiet time while an idle endpoint still owes a loss report.
    pub(super) fn loss_wait(
        &self,
        now: u64,
        timebase: Timebase,
        losses: (u64, u64),
    ) -> Option<Duration> {
        self.final_losses(losses).map(|_| {
            LOSS_QUIET.saturating_sub(Duration::from_nanos(
                timebase.nanoseconds(now.saturating_sub(self.changed_at)),
            ))
        })
    }

    /// The frames of an episode still open when the pump ends.
    pub(super) fn final_losses(&self, (skipped, dropped): (u64, u64)) -> Option<Losses> {
        let losses = Losses {
            skipped,
            dropped,
            stopped: self.stopped,
        };
        (losses != self.reported).then(|| losses.since(self.reported))
    }
}
