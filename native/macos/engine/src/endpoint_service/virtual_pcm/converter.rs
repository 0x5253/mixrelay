//! Leased source conversion keeps native timestamps and consumer feedback together.
#[cfg(test)]
mod tests;
use super::Error;
use mixrelay_audio_io::clock_bridge::{
    AsyncClockBridge, BridgeConfig, ClockFeedback, ClockRelation, Discontinuity, FeedbackQueue,
};
use mixrelay_macos_transport::application_source_transport::{
    ConsumerTimingRead, SourceClockSnapshot, SourceTransport,
};

/// Correction per frame of phase between a time-addressed consumer's request and the
/// next frame's timestamp. The measured period follows a change in the consumer's clock
/// once the phase passes half a frame; this term holds the phase inside that half frame
/// meanwhile, so a 200 ppm step or a 30 s ramp to 200 ppm costs at most two skipped
/// frames. A full-frame phase jump at a skipped frame moves the ratio by 120 ppm, far
/// below an audible pitch change.
const PHASE_PPM_PER_FRAME: f64 = 120.0;

/// Bridge input FIFO, in native frames. The pump pushes one period and flushes it before
/// the next, so the FIFO holds at most a period below the converter's input batch plus
/// that period. The size also covers a whole cycle of catch-up, priming and the sinc
/// history, so it never limits the pump's bounded feed.
pub(super) const BRIDGE_FRAMES: usize = 2_048;
/// Input frames converted before the first output, and the converter's input batch.
pub(super) const PRIMING_FRAMES: usize = 128;

pub(super) struct Converter {
    bridge: AsyncClockBridge,
    format: u64,
    rate: u32,
    master: bool,
    epoch: u64,
    ticks: u64,
    output: Vec<f32>,
    times: Vec<u64>,
    pending: usize,
    drain_limit: usize,
    /// The consumer last read by time.
    time_addressed: bool,
}
impl Converter {
    /// Prepares conversion storage for the requested format without touching the
    /// source; `restart` then commits its stream. A deferred acknowledgement reuses the
    /// storage, and a failed preparation leaves the source's epoch as it was.
    pub(super) fn new(clock: SourceClockSnapshot, native: (u32, usize)) -> Result<Self, Error> {
        let (rate, channels) = native;
        let output_rate = u32::try_from(clock.format_word & u64::from(u32::MAX)).unwrap_or(0);
        let bridge = AsyncClockBridge::new(BridgeConfig {
            channels,
            input_rate: rate,
            output_rate,
            max_input_frames: 128,
            max_output_frames: 128,
            capacity_frames: BRIDGE_FRAMES,
            priming_frames: PRIMING_FRAMES,
            clock_relation: if clock.clock_master != 0 {
                ClockRelation::ProvenSameClock
            } else {
                ClockRelation::Independent
            },
            feedback_queue: FeedbackQueue::Output,
            max_correction_ppm: 2_000.0,
        })?;
        let output =
            mixrelay_audio_io::pcm_buffer::zeroed(128 * channels).map_err(std::io::Error::other)?;
        let times =
            mixrelay_audio_io::pcm_buffer::timestamps(128).map_err(std::io::Error::other)?;
        Ok(Self {
            bridge,
            format: clock.format_word,
            rate,
            master: clock.clock_master != 0,
            // No stream yet: no source snapshot acknowledges epoch 0.
            epoch: 0,
            ticks: clock.consumer_ticks,
            output,
            times,
            pending: 0,
            time_addressed: false,
            // One flush converts at most a full FIFO; each pull moves 128 output frames.
            drain_limit: (BRIDGE_FRAMES * output_rate as usize)
                .div_ceil(rate.max(1) as usize * 128)
                + 2,
        })
    }
    /// Acknowledges the requested format and starts a new source epoch on this storage.
    /// Returns false when the acknowledgement defers.
    pub(super) fn restart(
        &mut self,
        transport: &mut SourceTransport,
        clock: SourceClockSnapshot,
    ) -> bool {
        let Some(epoch) = transport.begin_epoch(clock.format_word, self.rate) else {
            return false;
        };
        self.bridge.reset(epoch, Discontinuity::StreamRestart);
        self.epoch = epoch;
        self.ticks = clock.consumer_ticks;
        self.pending = 0;
        true
    }
    /// Whether `restart` can use this storage for the requested stream.
    pub(super) fn reusable(&self, clock: SourceClockSnapshot, rate: u32) -> bool {
        self.format == clock.format_word
            && self.rate == rate
            && self.master == (clock.clock_master != 0)
    }
    /// The drift correction now applied, in parts per million.
    #[cfg(test)]
    pub(super) fn correction_ppm(&self) -> f64 {
        self.bridge.correction_ppm()
    }
    pub(super) fn output_quantum(&self) -> usize {
        self.bridge.processing_quantum().1
    }
    pub(super) fn current(&self, clock: SourceClockSnapshot, rate: u32) -> bool {
        self.acknowledged(clock) && self.rate == rate && self.master == (clock.clock_master != 0)
    }
    pub(super) fn acknowledged(&self, clock: SourceClockSnapshot) -> bool {
        self.format == clock.format_word && self.epoch == clock.epoch
    }
    /// Steers the drift correction from the consumer. A consumer that reads in sequence
    /// drains the queue at its own clock, so the queue occupancy against its target
    /// carries the drift. One that reads by time keeps the occupancy level whatever the
    /// ratio, so its measured frame period sets the ratio directly, and a small phase
    /// term holds its next frame at its requested time. No integrator accumulates while
    /// a time-addressed consumer cannot respond through the queue.
    pub(super) fn observe(
        &mut self,
        transport: &SourceTransport,
        clock: SourceClockSnapshot,
    ) -> Result<(), Error> {
        // A snapshot from before this stream began describes the previous epoch's queue.
        if clock.epoch != self.epoch {
            return Ok(());
        }
        let elapsed = clock.consumer_ticks.saturating_sub(self.ticks);
        self.ticks = clock.consumer_ticks;
        let rate = self.format & u64::from(u32::MAX);
        match transport.consumer_timing() {
            ConsumerTimingRead::Timed(timing) => {
                #[expect(clippy::cast_precision_loss, reason = "graph rates below 2^24")]
                let nominal = 1e9 / rate as f64;
                let phase = timing.phase_ns.map_or(0.0, |phase| {
                    f64::from(phase) / nominal * PHASE_PPM_PER_FRAME
                });
                self.bridge
                    .set_clock_correction((nominal / timing.period_ns - 1.0) * 1e6 + phase)?;
                self.time_addressed = true;
                return Ok(());
            }
            // A time-addressed consumer mid-publication, or not yet reading this epoch,
            // keeps its last correction: its queue occupancy carries no drift.
            ConsumerTimingRead::Unknown if self.time_addressed => return Ok(()),
            ConsumerTimingRead::Unknown => {}
            ConsumerTimingRead::Sequential => self.time_addressed = false,
        }
        self.bridge.observe_clock(ClockFeedback {
            queue: FeedbackQueue::Output,
            epoch: self.epoch,
            queued_frames: usize::try_from(
                clock.produced_frames.saturating_sub(clock.consumed_frames),
            )
            .unwrap_or(usize::MAX),
            target_frames: clock.target_backlog_frames.max(1) as usize,
            elapsed_frames: usize::try_from(elapsed.min(rate)).unwrap_or(0),
            clock_running: clock.consumer_running != 0
                && clock.target_backlog_frames != 0
                && elapsed > 0
                && elapsed <= rate,
        })?;
        Ok(())
    }
    pub(super) fn flush(&mut self, transport: &mut SourceTransport) -> Result<bool, Error> {
        if !self.flush_pending(transport) {
            return Ok(false);
        }
        for _ in 0..self.drain_limit {
            let report = self
                .bridge
                .pull_timed_interleaved(&mut self.output, &mut self.times)?;
            self.pending = report.transferred_frames;
            if self.pending == 0 {
                return Ok(true);
            }
            if !self.flush_pending(transport) {
                return Ok(false);
            }
        }
        Ok(false)
    }
    fn flush_pending(&mut self, transport: &mut SourceTransport) -> bool {
        if self.pending == 0 {
            return true;
        }
        let channels = self.output.len() / 128;
        if !transport.write_clocked_pcm(
            &self.output[..self.pending * channels],
            u32::try_from(self.pending).unwrap_or(0),
            &self.times[..self.pending],
        ) {
            return false;
        }
        self.pending = 0;
        true
    }
    pub(super) fn push(
        &mut self,
        transport: &mut SourceTransport,
        input: &mut [f32],
        times: &[u64],
    ) -> Result<(), Error> {
        let faults = mixrelay_audio_io::pcm::sanitize_samples(input);
        if !transport.record_source_faults(self.epoch, faults) {
            return Err(Error::Stale);
        }
        self.bridge.push_timed_interleaved(input, times)?;
        Ok(())
    }
    /// Leaves converted output unwritten, as a failed publication would.
    #[cfg(test)]
    pub(super) fn hold_output(&mut self, frames: usize) {
        self.pending = frames;
    }
}
