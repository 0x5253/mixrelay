//! Allocation-free contract at the native driver / DSP boundary.
//!
//! A callback quantum is not the prepared maximum block size. A clock domain
//! identifies one sample timeline, not an OS or a nominal sample rate. Native
//! adapters own scheduling; this module never sleeps, opens devices, or resamples.

use crate::session::{ClockAuthority, SessionFormat};
use thiserror::Error;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingState {
    #[default]
    Idle,
    Running,
    Stopped,
}

/// Native evidence of source demand, never inferred from sample amplitude or a
/// stalled publication watermark. Unknown does not prove either health or idleness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputActivity {
    Unknown,
    Inactive,
    /// A newly admitted stream is filling its bounded startup window. This is
    /// not valid PCM and must never be used to conceal a previously running stall.
    Priming,
    Active,
}

/// Frames actually transferred by an adapter. Silence containing a complete
/// block is a successful transfer; unavailable input is not the same as silence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferOutcome {
    pub frames: u32,
    pub discontinuity: bool,
    pub activity: InputActivity,
    /// Replacements already made by a native input adapter before conversion.
    /// Report once even when this read has no available PCM; graph admission
    /// combines this with any replacements it performs itself.
    pub source_sample_faults: u64,
}

impl TransferOutcome {
    #[must_use]
    pub const fn complete(frames: u32) -> Self {
        Self {
            frames,
            discontinuity: false,
            activity: InputActivity::Active,
            source_sample_faults: 0,
        }
    }
}

/// Non-finite PCM replaced at prepared Core boundaries, without retaining samples or identities.
/// Counts describe replacements, not distinct source frames; `blocks` counts affected cycles once.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SampleFaults {
    /// Replacements while admitting external source PCM.
    pub source_samples: u64,
    /// Replacements after route accumulation, before a node consumes PCM.
    pub input_samples: u64,
    /// Replacements after a node produces PCM, including hosted processors.
    pub output_samples: u64,
    pub blocks: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProcessReport {
    /// Per-binding insufficient transfers, not inferred from sample amplitude.
    pub input_starvations: u64,
    pub output_overflows: u64,
    pub discontinuities: u64,
    pub sample_faults: SampleFaults,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockDomain {
    pub authority: ClockAuthority,
    /// Adapter-local identity; independent devices must not reuse an identity.
    pub id: u64,
    /// Increment after a native clock restart or format/timeline reset.
    pub epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessCycle {
    pub clock: ClockDomain,
    pub sample_rate: u32,
    /// Actual frames requested for this invocation, not buffer capacity.
    pub frames: u32,
    pub first_frame: u64,
    /// Native monotonic timestamp when available; absence must not be fabricated.
    pub monotonic_ns: Option<u64>,
    /// Explicit permission to start a new timeline after a discontinuity.
    pub discontinuity: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CycleError {
    #[error("cycle has an invalid rate, frame count, or sample position")]
    InvalidFormat,
    #[error("cycle sample rate differs from the prepared DSP rate")]
    SampleRateMismatch,
    #[error("native quantum exceeds the prepared DSP capacity")]
    CapacityExceeded,
    #[error("clock identity or sample timeline changed without a discontinuity")]
    ClockDiscontinuity,
    #[error("monotonic timestamp moved backwards")]
    TimestampRegressed,
}

#[derive(Debug, Default, Clone)]
pub struct CycleTracker {
    previous: Option<ProcessCycle>,
    /// Missing native timestamps do not erase the last observed host time. Frame
    /// continuity still advances through every accepted cycle independently.
    last_timestamp_ns: Option<u64>,
}

impl CycleTracker {
    /// Validate before consuming native input or producing output. Failed calls
    /// do not advance the timeline. Format changes require a newly prepared DSP
    /// instance; adapters must split oversized quantums with an explicit block
    /// adapter, never truncate them here.
    ///
    /// # Errors
    /// Returns a contract violation without changing this tracker.
    pub fn accept(
        &mut self,
        cycle: ProcessCycle,
        prepared: SessionFormat,
    ) -> Result<(), CycleError> {
        if cycle.sample_rate == 0
            || cycle.frames == 0
            || cycle
                .first_frame
                .checked_add(u64::from(cycle.frames))
                .is_none()
        {
            return Err(CycleError::InvalidFormat);
        }
        if cycle.sample_rate != prepared.sample_rate {
            return Err(CycleError::SampleRateMismatch);
        }
        if cycle.frames > prepared.block_frames {
            return Err(CycleError::CapacityExceeded);
        }
        if let Some(previous) = self.previous.filter(|_| !cycle.discontinuity) {
            if cycle.clock != previous.clock
                || cycle.first_frame != previous.first_frame + u64::from(previous.frames)
            {
                return Err(CycleError::ClockDiscontinuity);
            }
            if let (Some(now), Some(before)) = (cycle.monotonic_ns, self.last_timestamp_ns)
                && now < before
            {
                return Err(CycleError::TimestampRegressed);
            }
        }
        if cycle.discontinuity || cycle.monotonic_ns.is_some() {
            self.last_timestamp_ns = cycle.monotonic_ns;
        }
        self.previous = Some(cycle);
        Ok(())
    }
}

/// Transitional driver timeline. This is explicitly NOT a device-synchronized
/// clock. Each OS adapter uses it only while its native callback path is absent.
#[derive(Debug, Default)]
pub struct TimerTimeline {
    position: u64,
    epoch: u64,
}

/// Splits one validated native callback into prepared DSP blocks without losing
/// its clock identity, sample positions or timestamp. No audio is dropped when
/// the native quantum exceeds the graph's processing capacity.
pub struct CycleBlocks {
    cycle: ProcessCycle,
    capacity: u32,
    offset: u32,
}

impl CycleBlocks {
    /// # Errors
    /// Rejects invalid/rate-mismatched callbacks before any partial processing.
    pub fn new(cycle: ProcessCycle, prepared: SessionFormat) -> Result<Self, CycleError> {
        if prepared.block_frames == 0
            || cycle.frames == 0
            || cycle.sample_rate == 0
            || cycle
                .first_frame
                .checked_add(u64::from(cycle.frames))
                .is_none()
            || cycle.monotonic_ns.is_some_and(|time| {
                time.checked_add(
                    u64::from(cycle.frames) * 1_000_000_000 / u64::from(cycle.sample_rate),
                )
                .is_none()
            })
        {
            return Err(CycleError::InvalidFormat);
        }
        if cycle.sample_rate != prepared.sample_rate {
            return Err(CycleError::SampleRateMismatch);
        }
        Ok(Self {
            cycle,
            capacity: prepared.block_frames,
            offset: 0,
        })
    }
}

impl Iterator for CycleBlocks {
    type Item = ProcessCycle;
    fn next(&mut self) -> Option<Self::Item> {
        if self.offset == self.cycle.frames {
            return None;
        }
        let frames = self.capacity.min(self.cycle.frames - self.offset);
        let result = ProcessCycle {
            frames,
            first_frame: self.cycle.first_frame + u64::from(self.offset),
            monotonic_ns: self.cycle.monotonic_ns.map(|time| {
                time + u64::from(self.offset) * 1_000_000_000 / u64::from(self.cycle.sample_rate)
            }),
            discontinuity: self.cycle.discontinuity && self.offset == 0,
            ..self.cycle
        };
        self.offset += frames;
        Some(result)
    }
}

impl TimerTimeline {
    /// Called on pause/resume or native transport reset, outside DSP processing.
    pub fn reset(&mut self) {
        self.position = 0;
        self.epoch = self.epoch.wrapping_add(1);
    }

    #[must_use]
    pub fn next_cycle(&mut self, format: SessionFormat) -> ProcessCycle {
        let cycle = ProcessCycle {
            clock: ClockDomain {
                authority: ClockAuthority::EngineTimer,
                id: 0,
                epoch: self.epoch,
            },
            sample_rate: format.sample_rate,
            frames: format.block_frames,
            first_frame: self.position,
            monotonic_ns: None,
            discontinuity: self.position == 0,
        };
        if let Some(next) = self.position.checked_add(u64::from(format.block_frames)) {
            self.position = next;
        } else {
            self.reset();
        }
        cycle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const FORMAT: SessionFormat = SessionFormat {
        sample_rate: 48_000,
        block_frames: 512,
    };

    fn cycle(first_frame: u64, frames: u32) -> ProcessCycle {
        ProcessCycle {
            clock: ClockDomain {
                authority: ClockAuthority::AudioServer,
                id: 7,
                epoch: 1,
            },
            sample_rate: 48_000,
            frames,
            first_frame,
            monotonic_ns: Some(first_frame),
            discontinuity: false,
        }
    }

    #[test]
    fn native_quantum_can_vary_without_changing_prepared_capacity() {
        let mut tracker = CycleTracker::default();
        for (position, frames) in [(0, 32), (32, 128), (160, 512), (672, 64)] {
            assert_eq!(tracker.accept(cycle(position, frames), FORMAT), Ok(()));
        }
    }

    #[test]
    fn splits_native_quantum_without_losing_frames_or_clock_context() {
        let native = ProcessCycle {
            discontinuity: true,
            ..cycle(512, 1057)
        };
        let prepared = SessionFormat {
            block_frames: 128,
            ..FORMAT
        };
        let blocks = CycleBlocks::new(native, prepared)
            .unwrap()
            .collect::<Vec<_>>();
        assert_eq!(blocks.len(), 9);
        assert_eq!(blocks.iter().map(|b| b.frames).sum::<u32>(), 1057);
        assert_eq!(blocks.last().unwrap().frames, 33);
        let mut tracker = CycleTracker::default();
        for (index, block) in blocks.into_iter().enumerate() {
            assert_eq!(block.clock, native.clock);
            assert_eq!(block.discontinuity, index == 0);
            assert_eq!(block.first_frame, 512 + index as u64 * 128);
            assert_eq!(tracker.accept(block, prepared), Ok(()));
        }
    }

    #[test]
    fn invalid_native_quantum_is_rejected_before_any_sub_block() {
        for bad in [
            cycle(0, 0),
            cycle(u64::MAX, 128),
            ProcessCycle {
                monotonic_ns: Some(u64::MAX),
                ..cycle(0, 128)
            },
            ProcessCycle {
                sample_rate: 0,
                ..cycle(0, 128)
            },
        ] {
            assert!(matches!(
                CycleBlocks::new(bad, FORMAT),
                Err(CycleError::InvalidFormat)
            ));
        }
        assert!(matches!(
            CycleBlocks::new(
                cycle(0, 128),
                SessionFormat {
                    sample_rate: 44_100,
                    ..FORMAT
                }
            ),
            Err(CycleError::SampleRateMismatch)
        ));
        assert!(matches!(
            CycleBlocks::new(
                cycle(0, 128),
                SessionFormat {
                    block_frames: 0,
                    ..FORMAT
                }
            ),
            Err(CycleError::InvalidFormat)
        ));
    }

    #[test]
    fn invalid_cycles_do_not_advance_the_timeline() {
        let mut tracker = CycleTracker::default();
        tracker.accept(cycle(0, 128), FORMAT).unwrap();
        for (bad, error) in [
            (cycle(128, 0), CycleError::InvalidFormat),
            (cycle(128, 1024), CycleError::CapacityExceeded),
            (
                ProcessCycle {
                    sample_rate: 44_100,
                    ..cycle(128, 128)
                },
                CycleError::SampleRateMismatch,
            ),
            (cycle(129, 128), CycleError::ClockDiscontinuity),
            (cycle(u64::MAX, 128), CycleError::InvalidFormat),
        ] {
            assert_eq!(tracker.accept(bad, FORMAT), Err(error));
        }
        assert_eq!(tracker.accept(cycle(128, 128), FORMAT), Ok(()));
    }

    #[test]
    fn independent_clocks_and_backward_timestamps_need_explicit_boundaries() {
        let mut tracker = CycleTracker::default();
        tracker.accept(cycle(128, 128), FORMAT).unwrap();
        let mut next = cycle(256, 128);
        next.clock.id = 8;
        assert_eq!(
            tracker.accept(next, FORMAT),
            Err(CycleError::ClockDiscontinuity)
        );
        next.discontinuity = true;
        assert_eq!(tracker.accept(next, FORMAT), Ok(()));
        let backwards = ProcessCycle {
            first_frame: 384,
            monotonic_ns: Some(1),
            discontinuity: false,
            ..next
        };
        assert_eq!(
            tracker.accept(backwards, FORMAT),
            Err(CycleError::TimestampRegressed)
        );
    }

    #[test]
    fn missing_timestamps_preserve_the_watermark_without_inventing_elapsed_time() {
        let mut tracker = CycleTracker::default();
        tracker.accept(cycle(1_000, 128), FORMAT).unwrap();
        for first_frame in [1_128, 1_256] {
            tracker
                .accept(
                    ProcessCycle {
                        monotonic_ns: None,
                        ..cycle(first_frame, 128)
                    },
                    FORMAT,
                )
                .unwrap();
        }
        assert_eq!(
            tracker.accept(
                ProcessCycle {
                    monotonic_ns: Some(999),
                    ..cycle(1_384, 128)
                },
                FORMAT
            ),
            Err(CycleError::TimestampRegressed)
        );
        // Rejection consumes no frame position. Only native evidence is compared;
        // no nominal-rate extrapolation of absent timestamps is fabricated.
        tracker
            .accept(
                ProcessCycle {
                    monotonic_ns: Some(1_001),
                    ..cycle(1_384, 128)
                },
                FORMAT,
            )
            .unwrap();
        tracker
            .accept(
                ProcessCycle {
                    discontinuity: true,
                    monotonic_ns: None,
                    ..cycle(0, 128)
                },
                FORMAT,
            )
            .unwrap();
        assert_eq!(tracker.accept(cycle(128, 128), FORMAT), Ok(()));
    }

    #[test]
    fn timer_never_claims_a_native_clock_or_timestamp() {
        let mut timer = TimerTimeline::default();
        let first = timer.next_cycle(FORMAT);
        assert_eq!(first.clock.authority, ClockAuthority::EngineTimer);
        assert_eq!(first.monotonic_ns, None);
        assert_eq!(timer.next_cycle(FORMAT).first_frame, 512);
        timer.reset();
        let reset = timer.next_cycle(FORMAT);
        assert_ne!(reset.clock.epoch, first.clock.epoch);
        assert!(reset.discontinuity);
    }
}
