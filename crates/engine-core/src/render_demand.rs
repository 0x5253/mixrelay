//! Native render demand translated into bounded graph cycles. The cumulative
//! count is in negotiated stream frames, NOT `IAudioClock`'s opaque tick units.
//! A wakeup only prompts a recheck: it never authorizes timer-generated audio.

use mixrelay_audio_io::{
    process::{ClockDomain, ProcessCycle},
    session::ClockAuthority,
};

#[derive(Default)]
pub struct RenderDemandCursor {
    previous: Option<(u64, u64)>,
    pending_boundary: Boundary,
    last_boundary: Boundary,
    native_source: Option<(u64, u64)>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Boundary {
    #[default]
    Continuous,
    Reset,
    ClockHandoff,
}

impl RenderDemandCursor {
    pub fn reset(&mut self) {
        self.previous = None;
        self.pending_boundary = Boundary::Reset;
        self.last_boundary = Boundary::Continuous;
    }

    /// Whether the last returned cycle only changed native clock owners.
    ///
    /// Read immediately after `observe` returns a cycle. Unlike a restart or
    /// dropped catch-up interval, a handoff retains graph PCM history; each
    /// input still reports its own transport loss independently.
    #[must_use]
    pub fn last_cycle_is_clock_handoff(&self) -> bool {
        self.last_boundary == Boundary::ClockHandoff
    }

    /// Adopts a producer-published connection boundary before observing demand.
    ///
    /// The owner ID stays stable across reconnects; its nonzero connection epoch
    /// increases on each reconnect. The first native owner retains frames
    /// completed before its first poll, and a reconnect of the current owner
    /// retains frames completed since that connection began. Switching to a
    /// different owner (including returning to an earlier owner) baselines at
    /// `current_frame`, discarding that owner's historical backlog. Repeated
    /// calls for the current owner/epoch never rewind an already-consumed cursor.
    ///
    /// Returns `false` for zero IDs, an invalid frame boundary, or an older epoch
    /// from the current owner; callers must not observe demand after rejection.
    #[must_use]
    pub fn adopt_epoch_at(
        &mut self,
        owner_id: u64,
        stream_id: u64,
        first_frame: u64,
        current_frame: u64,
    ) -> bool {
        if owner_id == 0 || stream_id == 0 || first_frame > current_frame {
            return false;
        }

        let (baseline, boundary) = match self.native_source {
            None => (first_frame, Boundary::Reset),
            Some((current_owner, current_epoch)) if current_owner == owner_id => {
                if stream_id < current_epoch {
                    return false;
                }
                if stream_id == current_epoch {
                    return true;
                }
                (first_frame, Boundary::Reset)
            }
            Some(_) => (
                current_frame,
                if self.previous.is_some() && self.pending_boundary != Boundary::Reset {
                    Boundary::ClockHandoff
                } else {
                    Boundary::Reset
                },
            ),
        };

        self.native_source = Some((owner_id, stream_id));
        self.previous = Some((stream_id, baseline));
        self.pending_boundary = boundary;
        true
    }

    /// One graph owner consumes a monotonically increasing native demand count.
    /// On attachment discard preparation-time demand. On a delayed wakeup cap
    /// catch-up to the cycle budget, explicitly marking the lost interval; never
    /// synthesize an unbounded burst of obsolete audio.
    #[must_use]
    pub fn observe(
        &mut self,
        stream_id: u64,
        total_frames: u64,
        sample_rate: u32,
        max_cycle_frames: u32,
    ) -> Option<ProcessCycle> {
        if max_cycle_frames == 0 || sample_rate == 0 {
            return None;
        }
        let previous = self.previous.replace((stream_id, total_frames));
        let Some((id, position)) = previous.filter(|(id, _)| *id == stream_id) else {
            self.pending_boundary = Boundary::Reset;
            return None;
        };
        if total_frames < position {
            self.pending_boundary = Boundary::Reset;
            return None;
        }
        let pending = total_frames - position;
        if pending == 0 {
            return None;
        }
        let frames = u32::try_from(pending)
            .unwrap_or(max_cycle_frames)
            .min(max_cycle_frames);
        if pending > u64::from(max_cycle_frames) {
            self.pending_boundary = Boundary::Reset;
        }
        self.last_boundary = std::mem::take(&mut self.pending_boundary);
        Some(ProcessCycle {
            clock: ClockDomain {
                authority: ClockAuthority::Device,
                id,
                epoch: 0,
            },
            sample_rate,
            frames,
            first_frame: total_frames - u64::from(frames),
            // No wall-clock approximation masquerading as a native timestamp.
            monotonic_ns: None,
            discontinuity: self.last_boundary != Boundary::Continuous,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixrelay_audio_io::{
        process::{CycleBlocks, CycleTracker},
        session::CURRENT_TIMER_FORMAT,
    };

    #[test]
    fn only_lossless_owner_changes_preserve_history() {
        let mut cursor = RenderDemandCursor::default();
        assert!(cursor.adopt_epoch_at(1, 1, 0, 480));
        assert!(cursor.observe(1, 480, 48_000, 4096).is_some());
        assert!(!cursor.last_cycle_is_clock_handoff());

        assert!(cursor.adopt_epoch_at(2, 2, 0, 960));
        assert!(cursor.observe(2, 960, 48_000, 4096).is_none());
        let handoff = cursor.observe(2, 1440, 48_000, 4096).unwrap();
        assert!(handoff.discontinuity);
        assert!(cursor.last_cycle_is_clock_handoff());
        assert!(!cursor.observe(2, 1920, 48_000, 4096).unwrap().discontinuity);
        assert!(!cursor.last_cycle_is_clock_handoff());

        assert!(cursor.adopt_epoch_at(2, 3, 1920, 2400));
        assert!(cursor.observe(3, 2400, 48_000, 4096).unwrap().discontinuity);
        assert!(
            !cursor.last_cycle_is_clock_handoff(),
            "reconnect is a reset"
        );
        assert!(cursor.adopt_epoch_at(1, 1, 0, 3000));
        assert!(
            cursor
                .observe(1, 30_000, 48_000, 4096)
                .unwrap()
                .discontinuity
        );
        assert!(
            !cursor.last_cycle_is_clock_handoff(),
            "catch-up loss is a reset"
        );

        cursor.reset();
        assert!(cursor.adopt_epoch_at(2, 3, 1920, 2880));
        assert!(cursor.observe(3, 3360, 48_000, 4096).unwrap().discontinuity);
        assert!(
            !cursor.last_cycle_is_clock_handoff(),
            "idle/format reset wins"
        );
    }

    #[test]
    fn explicit_epoch_boundary_preserves_early_credit_without_replaying_old_frames() {
        let mut cursor = RenderDemandCursor::default();
        assert!(cursor.adopt_epoch_at(11, 1, 1000, 1128));
        assert_eq!(cursor.observe(1, 1128, 48_000, 4096).unwrap().frames, 128);
        assert!(cursor.adopt_epoch_at(11, 1, 1000, 1128));
        assert!(cursor.observe(1, 1128, 48_000, 4096).is_none());
        assert!(cursor.adopt_epoch_at(11, 2, 1128, 1192));
        let restarted = cursor.observe(2, 1192, 48_000, 4096).unwrap();
        assert_eq!(restarted.frames, 64);
        assert_eq!(restarted.first_frame, 1128);
        assert!(restarted.discontinuity);
    }

    #[test]
    fn reset_and_return_to_existing_clock_never_replay_its_connection_history() {
        let mut cursor = RenderDemandCursor::default();
        assert!(cursor.adopt_epoch_at(11, 1, 0, 1000));
        assert_eq!(cursor.observe(1, 1000, 48_000, 4096).unwrap().frames, 1000);
        cursor.reset();
        assert!(cursor.adopt_epoch_at(11, 1, 0, 1100));
        assert!(cursor.observe(1, 1100, 48_000, 4096).is_none());
        assert!(cursor.adopt_epoch_at(11, 1, 0, 1164));
        assert_eq!(cursor.observe(1, 1164, 48_000, 4096).unwrap().frames, 64);

        assert!(cursor.adopt_epoch_at(22, 2, 2000, 2064));
        assert!(cursor.observe(2, 2064, 48_000, 4096).is_none());
        assert_eq!(cursor.observe(2, 2128, 48_000, 4096).unwrap().frames, 64);
        cursor.reset();
        assert!(cursor.adopt_epoch_at(11, 1, 0, 1228));
        assert!(cursor.observe(1, 1228, 48_000, 4096).is_none());
        assert!(cursor.adopt_epoch_at(11, 1, 0, 1292));
        assert_eq!(cursor.observe(1, 1292, 48_000, 4096).unwrap().frames, 64);
    }

    #[test]
    fn owner_switch_and_return_baseline_existing_history() {
        let mut cursor = RenderDemandCursor::default();
        assert!(cursor.adopt_epoch_at(11, 1, 0, 480));
        assert_eq!(cursor.observe(1, 480, 48_000, 8192).unwrap().frames, 480);

        // B was already running before this cursor selected it. Its old frames
        // are not credit, even when they exceed the normal catch-up cap.
        assert!(cursor.adopt_epoch_at(22, 2, 0, 50_000));
        assert!(cursor.observe(2, 50_000, 48_000, 8192).is_none());
        assert_eq!(cursor.observe(2, 50_480, 48_000, 8192).unwrap().frames, 480);

        // A return is another source switch, not permission to replay A's past.
        assert!(cursor.adopt_epoch_at(11, 3, 480, 80_000));
        assert!(cursor.observe(3, 80_000, 48_000, 8192).is_none());
        assert_eq!(cursor.observe(3, 80_480, 48_000, 8192).unwrap().frames, 480);
    }

    #[test]
    fn invalid_or_stale_owner_epoch_adoption_does_not_mutate_the_cursor() {
        let mut cursor = RenderDemandCursor::default();
        assert!(!cursor.adopt_epoch_at(0, 1, 0, 480));
        assert!(!cursor.adopt_epoch_at(11, 0, 0, 480));
        assert!(!cursor.adopt_epoch_at(11, 1, 481, 480));
        assert!(cursor.adopt_epoch_at(11, 2, 0, 480));
        assert!(!cursor.adopt_epoch_at(11, 1, 0, 512));
        assert_eq!(cursor.observe(2, 480, 48_000, 4096).unwrap().frames, 480);
    }

    #[test]
    fn only_native_frames_authorize_processing_and_remainders_are_preserved() {
        let mut cursor = RenderDemandCursor::default();
        let mut tracker = CycleTracker::default();
        assert!(cursor.observe(1, 9_000, 48_000, 4096).is_none());
        assert!(cursor.observe(1, 9_000, 48_000, 4096).is_none());
        let mut total = 9_000;
        let mut processed = 0;
        for quantum in [480, 441, 960, 1, 127, 129]
            .into_iter()
            .cycle()
            .take(10_000)
        {
            total += quantum;
            let cycle = cursor.observe(1, total, 48_000, 4096).unwrap();
            for block in CycleBlocks::new(cycle, CURRENT_TIMER_FORMAT).unwrap() {
                tracker.accept(block, CURRENT_TIMER_FORMAT).unwrap();
                processed += u64::from(block.frames);
            }
        }
        assert_eq!(processed, total - 9_000);
    }

    #[test]
    fn full_admitted_bursts_are_not_truncated() {
        let mut cursor = RenderDemandCursor::default();
        assert!(cursor.observe(1, 0, 48_000, 8192).is_none());
        for burst in 0..4 {
            let cycle = cursor.observe(1, (burst + 1) * 8192, 48_000, 8192).unwrap();
            assert_eq!(cycle.frames, 8192);
            assert_eq!(cycle.first_frame, burst * 8192);
            assert_eq!(cycle.discontinuity, burst == 0);
        }
    }

    #[test]
    fn switch_stall_reset_and_counter_restart_are_explicit_discontinuities() {
        let mut cursor = RenderDemandCursor::default();
        assert!(cursor.observe(1, 0, 48_000, 4096).is_none());
        assert!(cursor.observe(1, 480, 48_000, 4096).unwrap().discontinuity);
        assert!(!cursor.observe(1, 960, 48_000, 4096).unwrap().discontinuity);
        let catchup = cursor.observe(1, 100_960, 48_000, 4096).unwrap();
        assert_eq!(catchup.frames, 4096);
        assert_eq!(catchup.first_frame, 96_864);
        assert!(catchup.discontinuity);
        assert!(cursor.observe(2, 3_000, 48_000, 4096).is_none());
        assert!(
            cursor
                .observe(2, 3_480, 48_000, 4096)
                .unwrap()
                .discontinuity
        );
        assert!(cursor.observe(2, 0, 48_000, 4096).is_none());
        assert!(cursor.observe(2, 480, 48_000, 4096).unwrap().discontinuity);
        cursor.reset();
        assert!(cursor.observe(2, 960, 48_000, 4096).is_none());
    }
}
