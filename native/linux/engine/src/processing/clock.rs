//! `PipeWire` adapters deliver the selected PCM rate even when the graph clock uses
//! another rate. Convert callback duration, not samples: doing SRC here again
//! would resample data that `PipeWire` already converted at the stream boundary.
use mixrelay_audio_io::process::{ClockDomain, CycleError, ProcessCycle};
use mixrelay_audio_io::session::ClockAuthority;
use mixrelay_linux_pipewire::DriverCycle;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Last successful native callback and the callback that changed its timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeDiscontinuity {
    pub(crate) previous: DriverCycle,
    pub(crate) current: DriverCycle,
    pub(crate) mapped_epoch: u64,
    pub(crate) render_position: u64,
}

/// Maps native callback duration while retaining the last successfully mapped callback.
pub(crate) struct GraphTimeline {
    mapper: mixrelay_audio_io::frame_budget::FrameBudgetMapper,
    previous: Option<DriverCycle>,
    pending: Option<NativeDiscontinuity>,
}

#[cfg(test)]
impl Default for GraphTimeline {
    fn default() -> Self {
        Self::new(crate::runtime::ENGINE_SAMPLE_RATE).expect("engine rate is nonzero")
    }
}

impl GraphTimeline {
    /// Creates a mapper for a render rate.
    ///
    /// # Errors
    /// A zero render rate cannot define a frame timeline.
    pub(crate) fn new(rate: u32) -> Result<Self, CycleError> {
        Ok(Self {
            mapper: mixrelay_audio_io::frame_budget::FrameBudgetMapper::new(rate)?,
            previous: None,
            pending: None,
        })
    }
    /// Maps one native callback; invalid rate, quantum, or arithmetic leaves state unchanged.
    pub(crate) fn map(&mut self, native: &DriverCycle) -> Result<ProcessCycle, CycleError> {
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
    pub(crate) fn take_discontinuity(&mut self) -> Option<NativeDiscontinuity> {
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
pub(crate) struct DiscontinuityMailbox {
    sequence: AtomicU64,
    previous: AtomicCycle,
    current: AtomicCycle,
    mapped_epoch: AtomicU64,
    render_position: AtomicU64,
}

/// Coherent latest event and count of overwritten events since the prior read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DiscontinuityRead {
    pub(crate) event_number: u64,
    pub(crate) overwritten: u64,
    pub(crate) event: NativeDiscontinuity,
}

impl DiscontinuityMailbox {
    /// Publishes from the serialized native callback without allocation or locking.
    pub(crate) fn publish(&self, event: NativeDiscontinuity) {
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
    pub(crate) fn read_since(&self, last_event: u64) -> Option<DiscontinuityRead> {
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
mod tests;
