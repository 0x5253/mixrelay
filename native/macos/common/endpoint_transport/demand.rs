use super::demand_atomics::{AtomicU32, AtomicU64, Ordering, fence};

/// One coherent callback deadline and quantum. Contending publishers defer
/// rather than waiting; readers never pair a new quantum with an old deadline.
#[repr(C)]
pub(super) struct Demand {
    pub(super) sequence: AtomicU64,
    stamp: AtomicU64,
    frames: AtomicU32,
    reserved: u32,
}

impl Demand {
    pub(super) fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            stamp: AtomicU64::new(0),
            frames: AtomicU32::new(0),
            reserved: 0,
        }
    }

    /// Called by the lifecycle owner only after local publishers have drained.
    /// The enclosing clock is UNREADY throughout this reset.
    pub(super) fn reset(&self) {
        self.stamp.store(0, Ordering::Relaxed);
        self.frames.store(0, Ordering::Relaxed);
        self.sequence.store(0, Ordering::Release);
    }

    pub(super) fn publish(&self, stamp: u64, frames: u32) {
        let sequence = self.sequence.load(Ordering::Relaxed);
        let Some(committed) = sequence.checked_add(2) else {
            return;
        };
        if sequence & 1 != 0
            || self
                .sequence
                .compare_exchange(sequence, sequence + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_err()
        {
            return;
        }
        // Pair with the reader fence: observing either replacement payload
        // orders the odd sequence before its final validation load.
        fence(Ordering::Release);
        if stamp > self.stamp.load(Ordering::Relaxed) {
            self.frames.store(frames, Ordering::Relaxed);
            self.stamp.store(stamp, Ordering::Relaxed);
        }
        self.sequence.store(committed, Ordering::Release);
    }

    pub(super) fn read(&self) -> Option<(u64, u32)> {
        let sequence = self.sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 {
            return None;
        }
        let stamp = self.stamp.load(Ordering::Relaxed);
        let frames = self.frames.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        (sequence == self.sequence.load(Ordering::Acquire)).then_some((stamp, frames))
    }
}
