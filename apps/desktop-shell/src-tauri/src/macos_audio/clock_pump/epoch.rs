//! One transport epoch's sample-rate converter and its engine clock feedback.
use super::ports::{CaptureTap, SourceSink};
use super::timing::{graph_frames, graph_rate};
use super::{ENGINE_BLOCK_FRAMES, PcmBlock, PumpError};
use crate::macos_audio::CAPTURE_RING_CAPACITY_FRAMES;
use mixrelay_audio_io::clock_bridge::{
    AsyncClockBridge, BridgeConfig, ClockFeedback, ClockRelation, Discontinuity, FeedbackQueue,
};
use mixrelay_macos_process_tap::{TapReadGeneration, TapStats};
use mixrelay_macos_transport::application_source_transport::SourceClockSnapshot;
use mixrelay_time::Instant;

/// Converted frames the bridge holds before it releases output.
const PRIMING_FRAMES: usize = 256;
/// The tap is not read while the converter already queues this many input frames.
const MAX_QUEUED_INPUT_FRAMES: usize = 256;

/// Converts tap PCM to the graph rate for one transport epoch. A new epoch starts when the
/// format, rate, clock role or native timeline changes; nothing carries across it.
pub(super) struct ConversionEpoch {
    pub(super) bridge: AsyncClockBridge,
    pub(super) format: u64,
    pub(super) native_rate: u32,
    ticks: u64,
    epoch: u64,
    dropped_frames: u64,
    pub(super) queued_input: usize,
    native_generation: u64,
    native_discontinuities: u64,
    clock_master: bool,
    measured_native_frames: u64,
    producer_burst: u32,
    publication_burst: u32,
    previous_publication: Option<Instant>,
}

impl ConversionEpoch {
    pub(super) fn new(
        channels: usize,
        rate: u32,
        consumer: SourceClockSnapshot,
        dropped_frames: u64,
    ) -> Result<Self, String> {
        let bridge = AsyncClockBridge::new(BridgeConfig {
            channels,
            input_rate: rate,
            output_rate: graph_rate(consumer.format_word),
            max_input_frames: ENGINE_BLOCK_FRAMES,
            max_output_frames: ENGINE_BLOCK_FRAMES,
            capacity_frames: CAPTURE_RING_CAPACITY_FRAMES,
            priming_frames: PRIMING_FRAMES,
            clock_relation: if consumer.clock_master != 0 {
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
            format: consumer.format_word,
            native_rate: rate,
            ticks: consumer.consumer_ticks,
            epoch: 0,
            dropped_frames,
            queued_input: 0,
            native_generation: 0,
            native_discontinuities: 0,
            clock_master: consumer.clock_master != 0,
            measured_native_frames: 0,
            producer_burst: 0,
            publication_burst: 0,
            previous_publication: None,
        })
    }

    /// Whether this epoch no longer describes the stream: a format, rate, clock role,
    /// ring overflow or native timestamp break ends it.
    pub(super) fn needs_reset(
        &self,
        consumer: SourceClockSnapshot,
        health: &TapStats,
        rate: u32,
    ) -> bool {
        self.format != consumer.format_word
            || self.native_rate != rate
            || self.dropped_frames != health.dropped_frames
            || self.native_generation != health.format_generation
            || self.native_discontinuities != health.clock_discontinuities
            || self.clock_master != (consumer.clock_master != 0)
    }

    /// Opens this epoch on the transport and discards the tap backlog left over from the
    /// previous one. `None` when the transport refuses the epoch.
    pub(super) fn start(
        mut self,
        capture: &mut impl CaptureTap,
        transport: &mut impl SourceSink,
        scratch: &mut [f32],
        health: &TapStats,
    ) -> Option<Self> {
        if !transport.reset(self.format, self.native_rate) {
            return None;
        }
        // Discard only the prefix present at the discontinuity, not an unbounded
        // drain while the capture callback continues producing new frames.
        let mut remaining = health.available_frames;
        while remaining > 0 {
            let frames = capture.read_interleaved(scratch);
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

    /// Feeds the engine's consumption into the converter's clock correction and returns
    /// the frames still queued in the shared ring.
    pub(super) fn observe(&mut self, consumer: SourceClockSnapshot) -> Result<usize, PumpError> {
        let queued = consumer
            .produced_frames
            .saturating_sub(consumer.consumed_frames);
        let queued = usize::try_from(queued).map_err(PumpError::Count)?;
        let elapsed = consumer.consumer_ticks.saturating_sub(self.ticks);
        self.ticks = consumer.consumer_ticks;
        // A stopped consumer does not turn clock-running merely because its last
        // published flag was true. No position advance means no PI update.
        let elapsed = usize::try_from(elapsed).map_err(PumpError::Count)?;
        let output_rate = usize::try_from(graph_rate(self.format)).map_err(PumpError::Count)?;
        self.bridge
            .observe_clock(ClockFeedback {
                queue: FeedbackQueue::Output,
                epoch: self.epoch,
                queued_frames: queued,
                target_frames: consumer.target_backlog_frames.max(1) as usize,
                elapsed_frames: elapsed.min(output_rate),
                clock_running: consumer.consumer_running != 0
                    && consumer.target_backlog_frames != 0
                    && elapsed > 0
                    && elapsed <= output_rate,
            })
            .map_err(PumpError::Bridge)?;
        Ok(queued)
    }

    /// Records the tap's newest burst and returns the producer and converter quanta to
    /// publish, in graph frames.
    pub(super) fn measure_input(&mut self, captured: u64) -> (u32, u32) {
        let native_burst = captured.saturating_sub(self.measured_native_frames);
        self.measured_native_frames = captured;
        self.producer_burst = self.producer_burst.max(graph_frames(
            native_burst,
            self.native_rate,
            graph_rate(self.format),
        ));
        // Published measurements saturate like the publication burst.
        let converter = u32::try_from(self.bridge.processing_quantum().1).unwrap_or(u32::MAX);
        (self.producer_burst, converter)
    }

    /// Records a publication and returns the largest burst so far and how long, in graph
    /// frames beyond one producer burst, the engine went without new PCM.
    pub(super) fn published(&mut self, transferred: usize, now: Instant) -> (u32, u32) {
        self.publication_burst = self
            .publication_burst
            .saturating_add(u32::try_from(transferred).unwrap_or(u32::MAX));
        let graph_rate = u128::from(graph_rate(self.format));
        let drought = self.previous_publication.map_or(0, |previous| {
            let elapsed = now
                .duration_since(previous)
                .as_nanos()
                .saturating_mul(graph_rate)
                .div_ceil(1_000_000_000);
            u32::try_from(elapsed)
                .unwrap_or(u32::MAX)
                .saturating_sub(self.producer_burst)
        });
        self.previous_publication = Some(now);
        (self.producer_burst.max(self.publication_burst), drought)
    }

    /// A pass that published nothing ends the current publication burst.
    pub(super) fn idle(&mut self) {
        self.publication_burst = 0;
    }

    /// Backpressure is neither producer jitter nor a coalesced publication.
    pub(super) fn backpressured(&mut self) {
        self.publication_burst = 0;
        self.previous_publication = None;
    }

    /// Whether the tap may be read now; a converter that already queues enough input
    /// drains before taking more.
    pub(super) fn accepts_input(&self) -> bool {
        self.queued_input < MAX_QUEUED_INPUT_FRAMES
    }

    /// Reads the tap, records sanitized faults, and converts. Returns the converted frames
    /// left in `output`.
    pub(super) fn transfer(
        &mut self,
        capture: &mut impl CaptureTap,
        transport: &mut impl SourceSink,
        input: &mut PcmBlock,
        output: &mut PcmBlock,
    ) -> Result<usize, PumpError> {
        let channels = capture.channels();
        let frames = if self.accepts_input() {
            capture.read_timed(
                &mut input.samples,
                &mut input.times,
                TapReadGeneration {
                    format: self.native_generation,
                    clock_breaks: self.native_discontinuities,
                },
            )
        } else {
            0
        };
        let samples = &mut input.samples[..frames * channels];
        let faults = mixrelay_audio_io::pcm::sanitize_samples(samples);
        if !transport.record_source_faults(self.epoch, faults) {
            return Ok(0);
        }
        if frames > 0 {
            self.bridge
                .push_timed_interleaved(samples, &input.times[..frames])
                .map_err(PumpError::Bridge)?;
        }
        let report = self
            .bridge
            .pull_timed_interleaved(&mut output.samples, &mut output.times)
            .map_err(PumpError::Bridge)?;
        self.queued_input = report.queued_input_frames;
        Ok(report.transferred_frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn consumer() -> SourceClockSnapshot {
        SourceClockSnapshot {
            format_word: (1_u64 << 32) | 0xbb80,
            ..SourceClockSnapshot::default()
        }
    }

    #[test]
    fn measured_native_coalesced_and_converter_bursts_are_graph_frames() {
        let mut epoch = ConversionEpoch::new(2, 48_000, consumer(), 0).unwrap();
        let (producer, converter) = epoch.measure_input(256);
        assert_eq!(producer, 256);
        assert!(converter >= 128);
        assert_eq!(epoch.measure_input(1280).0, 1024);
        assert_eq!(epoch.measure_input(1280).0, 1024);
        let now = Instant::from_origin(Duration::from_secs(1));
        assert_eq!(epoch.published(128, now), (1024, 0));
        assert_eq!(epoch.published(1024, now), (1152, 0));
        let (_, drought) = epoch.published(128, now.saturating_add(Duration::from_millis(30)));
        assert_eq!(drought, 416);
        epoch.backpressured();
        assert_eq!(
            epoch.published(128, now.saturating_add(Duration::from_secs(5))),
            (1024, 0),
            "backpressure is not a publication drought"
        );
    }

    #[test]
    fn published_target_changes_feedback_without_pcm_reset_and_inactive_freezes() {
        for master in [0, 1] {
            let mut snapshot = SourceClockSnapshot {
                produced_frames: 768,
                target_backlog_frames: 256,
                clock_master: master,
                consumer_running: 1,
                ..consumer()
            };
            let mut epoch = ConversionEpoch::new(2, 48_000, snapshot, 0).unwrap();
            epoch.bridge.push_interleaved(&[0.5; 256]).unwrap();
            for _ in 0..100 {
                snapshot.consumer_ticks += 128;
                epoch.observe(snapshot).unwrap();
            }
            let correction = epoch.bridge.correction_ppm();
            snapshot.target_backlog_frames = 1536;
            snapshot.consumer_running = 0;
            for _ in 0..100 {
                snapshot.consumer_ticks += 128;
                epoch.observe(snapshot).unwrap();
            }
            assert!((epoch.bridge.correction_ppm() - correction).abs() < f64::EPSILON);
            snapshot.consumer_running = 1;
            for _ in 0..1000 {
                snapshot.consumer_ticks += 128;
                epoch.observe(snapshot).unwrap();
            }
            if master == 0 {
                assert!(epoch.bridge.correction_ppm() > 0.0);
            } else {
                assert!(epoch.bridge.correction_ppm().abs() < f64::EPSILON);
            }
            assert_eq!(epoch.bridge.queued_input_frames(), 128);
            let correction = epoch.bridge.correction_ppm();
            snapshot.target_backlog_frames = 0; // stale/unknown consumer publication
            snapshot.consumer_ticks += 128;
            epoch.observe(snapshot).unwrap();
            assert!((epoch.bridge.correction_ppm() - correction).abs() < f64::EPSILON);
        }
        assert_eq!(
            std::mem::offset_of!(SourceClockSnapshot, target_backlog_frames),
            48
        );
        assert_eq!(
            std::mem::offset_of!(SourceClockSnapshot, starvation_episodes),
            56
        );
    }

    #[test]
    fn timestamp_break_resets_even_without_a_rate_change_or_ring_overflow() {
        let epoch = ConversionEpoch::new(2, 48_000, consumer(), 0).unwrap();
        let mut health = TapStats {
            callback_count: 1,
            captured_frames: 128,
            available_frames: 128,
            channels: 2,
            sample_rate: 48_000.0,
            running: true,
            format_ready: true,
            host_time: 100,
            clock_valid: true,
            ..TapStats::default()
        };
        assert!(!epoch.needs_reset(consumer(), &health, 48_000));
        health.host_time = 100_000; // observation time is not a rate-change signal
        assert!(!epoch.needs_reset(consumer(), &health, 48_000));
        health.clock_discontinuities = 1;
        assert!(epoch.needs_reset(consumer(), &health, 48_000));
    }

    #[test]
    fn only_engine_consumption_drives_feedback_and_master_never_corrects_itself() {
        for master in [0, 1] {
            let mut snapshot = SourceClockSnapshot {
                produced_frames: 1_024,
                consumer_running: 1,
                target_backlog_frames: 512,
                clock_master: master,
                ..consumer()
            };
            let mut epoch = ConversionEpoch::new(2, 48_000, snapshot, 0).unwrap();
            for _ in 0..1_000 {
                epoch.observe(snapshot).unwrap();
            }
            assert!(
                epoch.bridge.correction_ppm().abs() < f64::EPSILON,
                "worker ticks are not clock progress"
            );
            for _ in 0..1_000 {
                snapshot.consumer_ticks += 128;
                epoch.observe(snapshot).unwrap();
            }
            if master == 0 {
                assert!(epoch.bridge.correction_ppm() < 0.0);
            } else {
                assert!(epoch.bridge.correction_ppm().abs() < f64::EPSILON);
            }
        }
    }

    #[test]
    fn source_converter_transfers_rate_scaled_prefix_only() {
        for (native, graph) in [(44_100_u32, 48_000_u32), (96_000, 44_100), (48_000, 48_000)] {
            let snapshot = SourceClockSnapshot {
                format_word: (1_u64 << 32) | u64::from(graph),
                ..SourceClockSnapshot::default()
            };
            let mut epoch = ConversionEpoch::new(2, native, snapshot, 0).unwrap();
            let input = [0.25_f32; 256];
            let mut output = [0.0; 256];
            let mut valid = 0_usize;
            for _ in 0..1_000 {
                epoch.bridge.push_interleaved(&input).unwrap();
                loop {
                    let report = epoch.bridge.pull_interleaved(&mut output).unwrap();
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
        }
    }
}
