use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

struct Entry {
    stream: AtomicUsize,
    input: AtomicI32,
}

/// Fixed-capacity native identities; callbacks only read, lifecycle calls publish.
pub(crate) struct Directions<const N: usize>([Entry; N]);

impl<const N: usize> Directions<N> {
    pub(crate) const fn new() -> Self {
        Self(
            [const {
                Entry {
                    stream: AtomicUsize::new(0),
                    input: AtomicI32::new(-1),
                }
            }; N],
        )
    }

    pub(crate) fn open(&self, stream: usize) -> bool {
        if matches!(stream, 0 | usize::MAX) {
            return false;
        }
        for entry in &self.0 {
            if entry
                .stream
                .compare_exchange(0, usize::MAX, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                entry.input.store(-1, Ordering::Relaxed);
                entry.stream.store(stream, Ordering::Release);
                return true;
            }
        }
        false
    }

    pub(crate) fn connect(&self, stream: usize, input: bool) {
        if let Some(entry) = self.entry(stream) {
            entry.input.store(i32::from(input), Ordering::Release);
        }
    }

    pub(crate) fn input(&self, stream: usize) -> bool {
        self.entry(stream)
            .is_some_and(|entry| entry.input.load(Ordering::Acquire) == 1)
    }

    // Native destroy has drained callbacks before an identity can be reused.
    pub(crate) fn close(&self, stream: usize) {
        if let Some(entry) = self.entry(stream) {
            entry.stream.store(0, Ordering::Release);
        }
    }

    fn entry(&self, stream: usize) -> Option<&Entry> {
        if matches!(stream, 0 | usize::MAX) {
            return None;
        }
        self.0
            .iter()
            .find(|entry| entry.stream.load(Ordering::Acquire) == stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_connected_inputs_allow_dequeued_pcm_reads_even_after_reuse() {
        let directions = Directions::<1>::new();
        assert!(!directions.open(0));
        assert!(!directions.open(usize::MAX));
        assert!(!directions.input(0));
        assert!(directions.open(123));
        assert!(!directions.input(123));
        directions.connect(123, true);
        assert!(directions.input(123));
        assert!(!directions.open(456));
        directions.close(123);
        assert!(!directions.input(123));
        assert!(directions.open(123));
        assert!(!directions.input(123));
        directions.connect(123, false);
        assert!(!directions.input(123));
        let allocations = allocation_counter::measure(|| {
            directions.connect(123, true);
            std::hint::black_box(directions.input(123));
            directions.close(123);
        });
        assert_eq!(allocations.count_total, 0);
    }
}
