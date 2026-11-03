//! Fixed-capacity source coordinates, tagged by the PCM ring's absolute cursor.
use std::sync::atomic::{AtomicU64, Ordering, fence};

#[derive(Debug)]
struct Cell {
    stamp: AtomicU64,
    nanos: AtomicU64,
}

#[derive(Debug)]
pub(crate) struct FrameTimes {
    cells: Vec<Cell>,
}

impl FrameTimes {
    pub(crate) fn new(capacity: usize) -> Option<Self> {
        if capacity == 0 {
            return None;
        }
        let mut cells = Vec::new();
        cells.try_reserve_exact(capacity).ok()?;
        cells.resize_with(capacity, || Cell {
            stamp: AtomicU64::new(0),
            nanos: AtomicU64::new(0),
        });
        Some(Self { cells })
    }

    /// The PCM producer writes metadata before releasing the matching PCM range.
    pub(crate) fn publish(&self, cursor: u64, times: &[u64]) {
        for (offset, time) in times.iter().enumerate() {
            self.store(cursor.wrapping_add(offset as u64), *time);
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "bounded callback coordinates, invalid times publish unknown rather than relabel PCM"
    )]
    pub(crate) fn range(&self, cursor: u64, frames: usize, first: u64, period: f64) {
        for frame in 0..frames {
            let delta = period * frame as f64;
            let time = if first == 0 || !delta.is_finite() || delta < 0.0 {
                0
            } else {
                first.saturating_add(delta.round() as u64)
            };
            self.store(cursor.wrapping_add(frame as u64), time);
        }
    }

    fn store(&self, cursor: u64, nanos: u64) {
        let cell = &self.cells[usize::try_from(cursor % self.cells.len() as u64).unwrap_or(0)];
        cell.stamp.store(0, Ordering::Relaxed);
        // A reader observing a replacement coordinate must also observe invalidation.
        fence(Ordering::Release);
        cell.nanos.store(nanos, Ordering::Relaxed);
        cell.stamp.store(cursor.wrapping_add(1), Ordering::Release);
    }

    pub(crate) fn at(&self, cursor: u64) -> Option<u64> {
        let cell = &self.cells[usize::try_from(cursor % self.cells.len() as u64).unwrap_or(0)];
        let expected = cursor.wrapping_add(1);
        if expected == 0 || cell.stamp.load(Ordering::Acquire) != expected {
            return None;
        }
        let value = cell.nanos.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        (cell.stamp.load(Ordering::Acquire) == expected).then_some(value)
    }

    pub(crate) fn read(&self, cursor: u64, output: &mut [u64]) -> bool {
        let mut valid = true;
        for (offset, value) in output.iter_mut().enumerate() {
            *value = self
                .at(cursor.wrapping_add(offset as u64))
                .unwrap_or_else(|| {
                    valid = false;
                    0
                });
        }
        valid
    }

    pub(crate) fn locate(&self, start: u64, end: u64, nanos: u64) -> Option<u64> {
        let mut low = start;
        let mut high = end;
        while low < high {
            let middle = low + (high - low) / 2;
            if self
                .at(middle)
                .is_some_and(|time| time != 0 && time >= nanos)
            {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        if low > start
            && self.at(low - 1).is_some_and(|previous| {
                previous != 0
                    && previous.abs_diff(nanos)
                        < self.at(low).map_or(u64::MAX, |next| next.abs_diff(nanos))
            })
        {
            low -= 1;
        }
        (low < end).then_some(low)
    }
}

#[cfg(test)]
mod tests;
