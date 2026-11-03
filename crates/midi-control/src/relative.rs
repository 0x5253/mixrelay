//! Composed clipped translations preserve encoder reversals at the gain limits.
use std::sync::atomic::{AtomicU64, Ordering};

// Gain is represented in half-decibel units over the editor's 72dB range.
const RANGE: i16 = 144;
const IDENTITY: u64 = 256 | (144 << 17);
type Compare<'a> = dyn FnMut(&AtomicU64, u64, u64) -> Result<u64, u64> + 'a;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Movement(u64);

impl Movement {
    pub(crate) fn identity() -> Self {
        Self(IDENTITY)
    }

    fn append(self, delta: i16, sequence: u64) -> Self {
        let shift = self.shift().saturating_add(delta).clamp(-RANGE, RANGE);
        let low = self.bound(9).saturating_add(delta).clamp(0, RANGE);
        let high = self.bound(17).saturating_add(delta).clamp(0, RANGE);
        Self(
            u64::from((shift + 256).cast_unsigned())
                | (u64::from(low.cast_unsigned()) << 9)
                | (u64::from(high.cast_unsigned()) << 17)
                | (sequence << 25),
        )
    }

    fn shift(self) -> i16 {
        i16::try_from(self.0 & 511).unwrap_or_default() - 256
    }
    fn bound(self, shift: u32) -> i16 {
        i16::try_from((self.0 >> shift) & 255).unwrap_or_default()
    }

    pub(crate) fn sequence(self) -> u64 {
        self.0 >> 25
    }

    pub(crate) fn apply(self, gain: f32) -> f32 {
        (gain + f32::from(self.shift()) / 2.0).clamp(
            f32::from(self.bound(9)) / 2.0 - 60.0,
            f32::from(self.bound(17)) / 2.0 - 60.0,
        )
    }
}

#[derive(Debug)]
pub(crate) struct AtomicMovement(AtomicU64);

impl AtomicMovement {
    pub(crate) fn new() -> Self {
        Self(AtomicU64::new(IDENTITY))
    }

    pub(crate) fn append(&self, delta: i16, sequence: u64, reset: bool) {
        self.append_with(delta, sequence, reset, &mut |cell, old, next| {
            cell.compare_exchange(old, next, Ordering::AcqRel, Ordering::Acquire)
        });
    }

    fn append_with(&self, delta: i16, sequence: u64, reset: bool, compare: &mut Compare<'_>) {
        // One atomic publishes the whole transform. Acquire/Release pairs with
        // take, and retry work is bounded independently of the control worker.
        let mut old = self.0.load(Ordering::Acquire);
        for _ in 0..8 {
            let movement = if reset {
                Movement::identity()
            } else {
                Movement(old)
            };
            match compare(&self.0, old, movement.append(delta, sequence).0) {
                Ok(_) => return,
                Err(current) => old = current,
            }
        }
    }

    pub(crate) fn take(&self, sequence: u64) -> Movement {
        self.take_with(sequence, &mut |cell, old, next| {
            cell.compare_exchange(old, next, Ordering::AcqRel, Ordering::Acquire)
        })
    }

    fn take_with(&self, sequence: u64, compare: &mut Compare<'_>) -> Movement {
        // Consume only the captured receipt prefix. A newer transform remains intact,
        // including when cancellation replaced the aggregate after the raw-word check.
        let mut old = self.0.load(Ordering::Acquire);
        for _ in 0..8 {
            if crate::mailbox::reception_order(Movement(old).sequence(), sequence).is_gt() {
                return Movement::identity();
            }
            match compare(&self.0, old, IDENTITY) {
                Ok(_) => return Movement(old),
                Err(current) => old = current,
            }
        }
        Movement::identity()
    }
}

#[cfg(test)]
mod tests;
