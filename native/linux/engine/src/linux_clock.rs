//! `PipeWire` adapters deliver the selected PCM rate even when the graph clock uses
//! another rate. Convert callback duration, not samples: doing SRC here again
//! would resample data that `PipeWire` already converted at the stream boundary.
use mixrelay_audio_io::process::{ClockDomain, CycleError, ProcessCycle};
use mixrelay_audio_io::session::ClockAuthority;
use mixrelay_pipewire::DriverCycle;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Last successful native callback and the callback that changed its timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NativeDiscontinuity {
    pub(super) previous: DriverCycle,
    pub(super) current: DriverCycle,
    pub(super) mapped_epoch: u64,
    pub(super) render_position: u64,
}

/// Maps native callback duration while retaining the last successfully mapped callback.
pub(super) struct GraphTimeline {
    mapper: mixrelay_audio_io::frame_budget::FrameBudgetMapper,
    previous: Option<DriverCycle>,
    pending: Option<NativeDiscontinuity>,
}

#[cfg(test)]
impl Default for GraphTimeline {
    fn default() -> Self {
        Self::new(super::ENGINE_SAMPLE_RATE).expect("engine rate is nonzero")
    }
}

impl GraphTimeline {
    /// Creates a mapper for a render rate.
    ///
    /// # Errors
    /// A zero render rate cannot define a frame timeline.
    pub(super) fn new(rate: u32) -> Result<Self, CycleError> {
        Ok(Self {
            mapper: mixrelay_audio_io::frame_budget::FrameBudgetMapper::new(rate)?,
            previous: None,
            pending: None,
        })
    }
    /// Maps one native callback; invalid rate, quantum, or arithmetic leaves state unchanged.
    pub(super) fn map(&mut self, native: &DriverCycle) -> Result<ProcessCycle, CycleError> {
        // Bound callback work, reject malformed clocks before touching state.
        if !(8_000..=384_000).contains(&native.rate)
            || native.frames == 0
            || native.frames > DriverCycle::MAX_FRAMES
        {
            return Err(CycleError::InvalidFormat);
        }
        let cycle = self.mapper.map(ProcessCycle {
            clock: ClockDomain {
                authority: ClockAuthority::AudioServer,
                id: u64::from(native.clock_id),
                epoch: 1,
            },
            sample_rate: native.rate,
            frames: native.frames,
            first_frame: native.position,
            monotonic_ns: Some(native.monotonic_ns),
            discontinuity: false,
        })?;
        self.pending = if cycle.discontinuity && cycle.clock.epoch > 1 {
            self.previous
                .filter(|previous| {
                    previous.clock_id != native.clock_id
                        || previous.rate != native.rate
                        || previous.position.checked_add(u64::from(previous.frames))
                            != Some(native.position)
                })
                .map(|previous| NativeDiscontinuity {
                    previous,
                    current: *native,
                    mapped_epoch: cycle.clock.epoch,
                    render_position: cycle.first_frame,
                })
        } else {
            None
        };
        self.previous = Some(*native);
        Ok(cycle)
    }

    /// Consumes the latest mapped native transition at most once.
    pub(super) fn take_discontinuity(&mut self) -> Option<NativeDiscontinuity> {
        self.pending.take()
    }
}

#[derive(Default)]
struct AtomicCycle {
    position: AtomicU64,
    monotonic_ns: AtomicU64,
    clock_id: AtomicU32,
    rate: AtomicU32,
    frames: AtomicU32,
}

impl AtomicCycle {
    fn store(&self, cycle: DriverCycle) {
        self.position.store(cycle.position, Ordering::SeqCst);
        self.monotonic_ns
            .store(cycle.monotonic_ns, Ordering::SeqCst);
        self.clock_id.store(cycle.clock_id, Ordering::SeqCst);
        self.rate.store(cycle.rate, Ordering::SeqCst);
        self.frames.store(cycle.frames, Ordering::SeqCst);
    }

    fn load(&self) -> DriverCycle {
        DriverCycle {
            position: self.position.load(Ordering::SeqCst),
            monotonic_ns: self.monotonic_ns.load(Ordering::SeqCst),
            clock_id: self.clock_id.load(Ordering::SeqCst),
            rate: self.rate.load(Ordering::SeqCst),
            frames: self.frames.load(Ordering::SeqCst),
        }
    }
}

/// Single-writer, latest-only diagnostic mailbox shared with the non-RT control loop.
#[derive(Default)]
pub(super) struct DiscontinuityMailbox {
    sequence: AtomicU64,
    previous: AtomicCycle,
    current: AtomicCycle,
    mapped_epoch: AtomicU64,
    render_position: AtomicU64,
}

/// Coherent latest event and count of overwritten events since the prior read.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct DiscontinuityRead {
    pub(super) event_number: u64,
    pub(super) overwritten: u64,
    pub(super) event: NativeDiscontinuity,
}

impl DiscontinuityMailbox {
    /// Publishes from the serialized native callback without allocation or locking.
    pub(super) fn publish(&self, event: NativeDiscontinuity) {
        // One native callback writer marks the fixed atomic payload unstable, then stable.
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst);
        self.previous.store(event.previous);
        self.current.store(event.current);
        self.mapped_epoch
            .store(event.mapped_epoch, Ordering::SeqCst);
        self.render_position
            .store(event.render_position, Ordering::SeqCst);
        self.sequence
            .store(sequence.wrapping_add(2), Ordering::SeqCst);
    }

    /// Makes one bounded attempt to read an event newer than `last_event`.
    /// A concurrent write returns `None`; the next control tick can observe its latest event.
    pub(super) fn read_since(&self, last_event: u64) -> Option<DiscontinuityRead> {
        // One bounded attempt: if a write overlaps this read, the next control tick tries again.
        let before = self.sequence.load(Ordering::SeqCst);
        if before == 0 || before & 1 != 0 || before / 2 <= last_event {
            return None;
        }
        let event = NativeDiscontinuity {
            previous: self.previous.load(),
            current: self.current.load(),
            mapped_epoch: self.mapped_epoch.load(Ordering::SeqCst),
            render_position: self.render_position.load(Ordering::SeqCst),
        };
        if self.sequence.load(Ordering::SeqCst) != before {
            return None;
        }
        let event_number = before / 2;
        Some(DiscontinuityRead {
            event_number,
            overwritten: event_number.saturating_sub(last_event.saturating_add(1)),
            event,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::ENGINE_SAMPLE_RATE;
    use super::*;

    #[test]
    fn rate_mapping_preserves_exact_duration_with_variable_quantums() {
        for rate in [44_100, 48_000, 96_000] {
            let mut timeline = GraphTimeline::default();
            let mut input = 0;
            let mut output = 0;
            for quantum in [128, 256, 1024, 127].into_iter().cycle().take(10_000) {
                let cycle = timeline
                    .map(&DriverCycle {
                        position: input,
                        monotonic_ns: 0,
                        clock_id: 1,
                        rate,
                        frames: quantum,
                    })
                    .unwrap();
                assert_eq!(cycle.first_frame, output);
                assert_eq!(cycle.discontinuity, input == 0);
                input += u64::from(quantum);
                output += u64::from(cycle.frames);
            }
            assert_eq!(
                output,
                input * u64::from(ENGINE_SAMPLE_RATE) / u64::from(rate)
            );
        }
    }

    #[test]
    fn clock_change_reanchors_remainder_and_invalid_callback_preserves_state() {
        let mut timeline = GraphTimeline::default();
        let native = DriverCycle {
            position: 0,
            monotonic_ns: 0,
            clock_id: 1,
            rate: 44_100,
            frames: 128,
        };
        let first = timeline.map(&native).unwrap();
        assert!(timeline.map(&DriverCycle { rate: 0, ..native }).is_err());
        let changed = timeline
            .map(&DriverCycle {
                clock_id: 2,
                ..native
            })
            .unwrap();
        assert_eq!(first.frames, changed.frames);
        assert!(changed.discontinuity);
        assert_eq!(changed.clock.epoch, first.clock.epoch + 1);
        assert_eq!(changed.first_frame, u64::from(first.frames));
    }

    fn native(position: u64, clock_id: u32, rate: u32) -> DriverCycle {
        DriverCycle {
            position,
            monotonic_ns: position,
            clock_id,
            rate,
            frames: 128,
        }
    }

    fn event(number: u64) -> NativeDiscontinuity {
        NativeDiscontinuity {
            previous: native(number * 128, 1, 48_000),
            current: native(number * 128 + 128, 2, 44_100),
            mapped_epoch: number + 1,
            render_position: number * 128,
        }
    }

    #[test]
    fn native_timeline_events_capture_gap_clock_and_rate_changes() {
        let mut timeline = GraphTimeline::default();
        timeline.map(&native(0, 1, 48_000)).unwrap();
        assert_eq!(timeline.take_discontinuity(), None);
        timeline.map(&native(128, 1, 48_000)).unwrap();
        assert_eq!(timeline.take_discontinuity(), None);

        let gap = timeline.map(&native(512, 1, 48_000)).unwrap();
        assert_eq!(gap.clock.epoch, 2);
        assert_eq!(
            timeline.take_discontinuity(),
            Some(NativeDiscontinuity {
                previous: native(128, 1, 48_000),
                current: native(512, 1, 48_000),
                mapped_epoch: 2,
                render_position: 256,
            })
        );
        assert_eq!(timeline.take_discontinuity(), None);

        timeline.map(&native(640, 2, 48_000)).unwrap();
        let changed_clock = timeline.take_discontinuity().unwrap();
        assert_eq!(changed_clock.previous, native(512, 1, 48_000));
        assert_eq!(changed_clock.current, native(640, 2, 48_000));
        assert_eq!(changed_clock.mapped_epoch, 3);
        timeline.map(&native(768, 2, 44_100)).unwrap();
        let changed_rate = timeline.take_discontinuity().unwrap();
        assert_eq!(changed_rate.previous, native(640, 2, 48_000));
        assert_eq!(changed_rate.current, native(768, 2, 44_100));
        assert_eq!(changed_rate.mapped_epoch, 4);
    }

    #[test]
    fn invalid_native_cycle_does_not_advance_diagnostic_previous() {
        let mut timeline = GraphTimeline::default();
        timeline.map(&native(0, 1, 48_000)).unwrap();
        assert!(timeline.map(&native(128, 1, 0)).is_err());
        timeline.map(&native(128, 1, 48_000)).unwrap();
        assert_eq!(timeline.take_discontinuity(), None);
        assert!(timeline.map(&native(u64::MAX, 1, 48_000)).is_err());
        timeline.map(&native(512, 1, 48_000)).unwrap();
        assert_eq!(
            timeline.take_discontinuity().unwrap().previous,
            native(128, 1, 48_000)
        );
    }

    #[test]
    fn zero_frame_mapper_pending_discontinuity_is_reported_only_once() {
        let mut timeline = GraphTimeline::new(8_000).unwrap();
        let native = DriverCycle {
            position: 0,
            monotonic_ns: 0,
            clock_id: 1,
            rate: 192_000,
            frames: 1,
        };
        assert_eq!(timeline.map(&native).unwrap().frames, 0);
        assert_eq!(timeline.take_discontinuity(), None);

        let gap = DriverCycle {
            position: 1_000,
            ..native
        };
        let mapped_gap = timeline.map(&gap).unwrap();
        assert_eq!(mapped_gap.frames, 0);
        assert_eq!(mapped_gap.clock.epoch, 2);
        assert_eq!(timeline.take_discontinuity().unwrap().current, gap);

        let mut saw_render_frame = false;
        for position in 1_001..1_030 {
            let contiguous = DriverCycle { position, ..native };
            let cycle = timeline.map(&contiguous).unwrap();
            saw_render_frame |= cycle.frames > 0;
            assert_eq!(timeline.take_discontinuity(), None);
        }
        assert!(saw_render_frame);
    }

    #[test]
    fn mailbox_reports_overwrite_and_suppresses_duplicate_read() {
        let mailbox = DiscontinuityMailbox::default();
        assert_eq!(mailbox.read_since(0), None);
        mailbox.publish(event(1));
        assert_eq!(
            mailbox.read_since(0),
            Some(DiscontinuityRead {
                event_number: 1,
                overwritten: 0,
                event: event(1),
            })
        );
        assert_eq!(mailbox.read_since(1), None);
        mailbox.publish(event(2));
        mailbox.publish(event(3));
        assert_eq!(
            mailbox.read_since(1),
            Some(DiscontinuityRead {
                event_number: 3,
                overwritten: 1,
                event: event(3),
            })
        );
    }

    #[test]
    fn mailbox_snapshot_is_coherent_during_single_writer_updates() {
        use std::sync::Arc;

        let mailbox = Arc::new(DiscontinuityMailbox::default());
        let writer = Arc::clone(&mailbox);
        let worker = std::thread::spawn(move || {
            for number in 1..=10_000 {
                writer.publish(event(number));
                if number % 8 == 0 {
                    std::thread::yield_now();
                }
            }
        });
        let mut last = 0;
        for _ in 0..10_000 {
            if let Some(read) = mailbox.read_since(last) {
                assert_eq!(read.event, event(read.event_number));
                assert_eq!(read.overwritten, read.event_number - last - 1);
                last = read.event_number;
            }
            std::thread::yield_now();
        }
        worker.join().unwrap();
        if let Some(read) = mailbox.read_since(last) {
            assert_eq!(read.event, event(read.event_number));
            last = read.event_number;
        }
        assert_eq!(last, 10_000);
    }
}
