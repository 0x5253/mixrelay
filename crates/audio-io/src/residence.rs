//! Single-writer, cumulative measurements. Readers never reset callback counters.
use std::sync::atomic::{AtomicU64, Ordering};

/// A coherent cumulative sum and weight, with invalid observations counted separately.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ResidenceSnapshot {
    pub sum: u64,
    pub weight: u64,
    pub invalid: u64,
}
impl ResidenceSnapshot {
    /// Mean over the interval between two snapshots. Counter wrap is intentional.
    #[must_use]
    pub fn mean_since(self, previous: Self) -> Option<f64> {
        let weight = self.weight.wrapping_sub(previous.weight);
        if self.invalid != previous.invalid || weight == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        Some(self.sum.wrapping_sub(previous.sum) as f64 / weight as f64)
    }
}

/// One serialized consumer writes a tap; a control reader gets a bounded snapshot.
/// All callback operations are atomics only, with no allocation, locks or native calls.
#[derive(Debug, Default)]
pub struct Residence {
    sequence: AtomicU64,
    sum: AtomicU64,
    weight: AtomicU64,
    invalid: AtomicU64,
    /// Publisher-only: the current source's last weight and the mirror values it continues.
    source: Source,
}

#[derive(Debug, Default)]
struct Source {
    weight: AtomicU64,
    base_sum: AtomicU64,
    base_weight: AtomicU64,
    base_invalid: AtomicU64,
}

impl Residence {
    /// Mirror an exclusively owned converter's cumulative tap to its retained adapter.
    /// A replacement converter's tap restarts from zero, so a weight below the last
    /// published one is a new source (a real `u64` frame count never wraps). The mirror
    /// continues from its last value and counts that break as invalid: only the window
    /// spanning the replacement is skipped, and no reader sees cumulative values regress.
    pub fn publish(&self, value: ResidenceSnapshot) {
        // Relaxed: `source` is read and written only by this serialized publisher.
        let source = &self.source;
        if value.weight < source.weight.load(Ordering::Relaxed) {
            source
                .base_sum
                .store(self.sum.load(Ordering::Relaxed), Ordering::Relaxed);
            source
                .base_weight
                .store(self.weight.load(Ordering::Relaxed), Ordering::Relaxed);
            source.base_invalid.store(
                self.invalid.load(Ordering::Relaxed).wrapping_add(1),
                Ordering::Relaxed,
            );
        }
        source.weight.store(value.weight, Ordering::Relaxed);
        let base =
            |field: &AtomicU64, value: u64| field.load(Ordering::Relaxed).wrapping_add(value);
        self.sequence.fetch_add(1, Ordering::SeqCst);
        self.sum
            .store(base(&source.base_sum, value.sum), Ordering::SeqCst);
        self.weight
            .store(base(&source.base_weight, value.weight), Ordering::SeqCst);
        self.invalid
            .store(base(&source.base_invalid, value.invalid), Ordering::SeqCst);
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }
    pub fn record(&self, value: Option<u64>, weight: u64) {
        self.sequence.fetch_add(1, Ordering::SeqCst);
        if let Some(value) = value {
            self.sum
                .fetch_add(value.wrapping_mul(weight), Ordering::SeqCst);
            self.weight.fetch_add(weight, Ordering::SeqCst);
        } else {
            self.invalid.fetch_add(1, Ordering::SeqCst);
        }
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }

    #[must_use]
    pub fn snapshot(&self) -> Option<ResidenceSnapshot> {
        let before = self.sequence.load(Ordering::SeqCst);
        let value = ResidenceSnapshot {
            sum: self.sum.load(Ordering::SeqCst),
            weight: self.weight.load(Ordering::SeqCst),
            invalid: self.invalid.load(Ordering::SeqCst),
        };
        (before & 1 == 0 && self.sequence.load(Ordering::SeqCst) == before).then_some(value)
    }
}

/// One FIFO side's cumulative passage: the ring cursor this side has moved to and the
/// host-time area `sum(frames * host ticks)` of the frames it moved past (wrapping).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EdgeSnapshot {
    pub position: u64,
    pub area: u64,
    /// Host ticks of this side's latest passage.
    pub latest: u64,
    /// Operations that moved past frames without transferring them.
    pub discarded: u64,
}

/// Written only by the FIFO side's serialized role, with atomics only.
#[derive(Debug, Default)]
pub struct Edge {
    sequence: AtomicU64,
    position: AtomicU64,
    area: AtomicU64,
    latest: AtomicU64,
    discarded: AtomicU64,
}
impl Edge {
    /// Records this side's ring cursor after an operation at `host` ticks. Frames passed
    /// beyond `transferred` were discarded unread: they pass at `host` and invalidate the window.
    pub fn advance(&self, position: u64, transferred: u64, host: u64) {
        let passed = position.wrapping_sub(self.position.load(Ordering::SeqCst));
        if passed == 0 {
            return;
        }
        self.sequence.fetch_add(1, Ordering::SeqCst);
        self.position.store(position, Ordering::SeqCst);
        self.area
            .fetch_add(passed.wrapping_mul(host), Ordering::SeqCst);
        self.latest.store(host, Ordering::SeqCst);
        if passed != transferred {
            self.discarded.fetch_add(1, Ordering::SeqCst);
        }
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }

    #[must_use]
    pub fn snapshot(&self) -> Option<EdgeSnapshot> {
        let before = self.sequence.load(Ordering::SeqCst);
        let value = EdgeSnapshot {
            position: self.position.load(Ordering::SeqCst),
            area: self.area.load(Ordering::SeqCst),
            latest: self.latest.load(Ordering::SeqCst),
            discarded: self.discarded.load(Ordering::SeqCst),
        };
        (before & 1 == 0 && self.sequence.load(Ordering::SeqCst) == before).then_some(value)
    }
}

/// Host-time residence of one FIFO, timed by the side that actually moves each frame.
/// Unlike a fill sample, this stays exact when producers write whole blocks at once.
#[derive(Debug, Default)]
pub struct Passage {
    pub producer: Edge,
    pub consumer: Edge,
}
impl Passage {
    /// Both sides at one instant, or `None` when either moved meanwhile: frames the
    /// consumer passed after its sample would otherwise count as queued.
    #[must_use]
    pub fn snapshot(&self) -> Option<PassageSnapshot> {
        let consumer = self.consumer.snapshot()?;
        let producer = self.producer.snapshot()?;
        coherent(consumer, producer, self.consumer.snapshot()?)
    }
}

fn coherent(
    consumer: EdgeSnapshot,
    producer: EdgeSnapshot,
    after: EdgeSnapshot,
) -> Option<PassageSnapshot> {
    // A consumer can also pass frames before their producer records them.
    (after == consumer && u32::try_from(producer.position.wrapping_sub(consumer.position)).is_ok())
        .then_some(PassageSnapshot { producer, consumer })
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PassageSnapshot {
    pub producer: EdgeSnapshot,
    pub consumer: EdgeSnapshot,
}
impl PassageSnapshot {
    /// Mean host ticks between production and consumption of the frames consumed since
    /// `previous`. Frames still queued at either edge are counted as produced at that
    /// edge's latest production, which is exact while they are all from that write.
    /// Otherwise the error is at most `queued * (latest - oldest queued write) / frames`
    /// per edge, with opposite signs at the two edges, so it vanishes in steady state.
    #[must_use]
    pub fn residence_since(self, previous: Self) -> Option<f64> {
        let frames = self
            .consumer
            .position
            .wrapping_sub(previous.consumer.position);
        if self.consumer.discarded != previous.consumer.discarded || frames == 0 {
            return None;
        }
        let queued = |edge: Self| {
            edge.producer
                .position
                .wrapping_sub(edge.consumer.position)
                .wrapping_mul(edge.producer.latest)
        };
        // Each term wraps, but their combination is a small signed area.
        let ticks = (self.consumer.area.wrapping_sub(previous.consumer.area))
            .wrapping_sub(self.producer.area.wrapping_sub(previous.producer.area))
            .wrapping_add(queued(self))
            .wrapping_sub(queued(previous))
            .cast_signed();
        #[allow(clippy::cast_precision_loss)]
        Some(ticks as f64 / frames as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_weight_partial_reads_and_invalid_evidence_without_resetting_writer() {
        let tap = Residence::default();
        let empty = tap.snapshot().unwrap();
        assert_eq!(empty.mean_since(empty), None);
        let measured = allocation_counter::measure(|| {
            tap.record(Some(180), 20);
            tap.record(Some(120), 40);
        });
        assert_eq!(measured.count_total, 0);
        let first = tap.snapshot().unwrap();
        assert_eq!(first.mean_since(empty), Some(140.0));
        let mirror = Residence::default();
        mirror.publish(first);
        assert_eq!(mirror.snapshot(), Some(first));
        tap.record(Some(6), 1);
        assert_eq!(tap.snapshot().unwrap().mean_since(first), Some(6.0));
        tap.record(None, 1);
        assert_eq!(tap.snapshot().unwrap().mean_since(first), None);
        tap.sequence.store(1, Ordering::SeqCst);
        assert_eq!(tap.snapshot(), None);
        let wrapped = ResidenceSnapshot {
            sum: 3,
            weight: 1,
            invalid: 0,
        };
        assert_eq!(
            wrapped.mean_since(ResidenceSnapshot {
                sum: u64::MAX,
                weight: u64::MAX,
                invalid: 0
            }),
            Some(2.0)
        );
    }

    /// A replacement converter restarts its tap from zero. Mirrored raw, the window across
    /// it wraps both differences and a 1024-frame mean reads as about one frame.
    #[test]
    fn a_replacement_converter_skips_only_the_window_that_spans_it() {
        let converter = |frames| ResidenceSnapshot {
            sum: 1_024 * frames,
            weight: frames,
            invalid: 0,
        };
        let mirror = Residence::default();
        mirror.publish(converter(1_000_000));
        let before = mirror.snapshot().unwrap();
        let measured = allocation_counter::measure(|| mirror.publish(converter(4_800)));
        assert_eq!(measured.count_total, 0);
        let spanning = mirror.snapshot().unwrap();
        assert_eq!(spanning.mean_since(before), None);
        assert!(spanning.weight > before.weight && spanning.sum > before.sum);
        mirror.publish(converter(9_600));
        let after = mirror.snapshot().unwrap();
        assert_eq!(after.mean_since(spanning), Some(1_024.0));
        // The replaced converter's own invalid count restarts too; that cannot hide the break.
        mirror.publish(ResidenceSnapshot {
            invalid: 1,
            ..converter(9_700)
        });
        let flagged = mirror.snapshot().unwrap();
        mirror.publish(converter(100));
        assert_eq!(mirror.snapshot().unwrap().mean_since(flagged), None);
    }

    /// Whole blocks written at once and read one period later: every frame waits a full
    /// period. A fill sample at the read (`2 * 256 - 256`) halved that to 128 ticks.
    #[test]
    fn passage_times_burst_writes_exactly_across_wrapping_host_areas() {
        let tap = Passage::default();
        let base = u64::MAX / 3;
        let (mut written, mut read) = (0, 0);
        let mut block = |index: u64| {
            written += 256;
            tap.producer.advance(written, 256, base + index * 256);
            if index != 0 {
                read += 256;
                tap.consumer.advance(read, 256, base + index * 256);
            }
        };
        let measured = allocation_counter::measure(|| block(0));
        assert_eq!(measured.count_total, 0);
        let first = tap.snapshot().unwrap();
        for index in 1..=8 {
            block(index);
        }
        let second = tap.snapshot().unwrap();
        assert_eq!(second.residence_since(first), Some(256.0));
        assert_eq!(second.residence_since(second), None);
        // A window edge inside a block: queued frames are all from the latest write.
        tap.consumer.advance(read + 100, 100, base + 9 * 256 - 156);
        let partial = tap.snapshot().unwrap();
        assert_eq!(partial.residence_since(second), Some(100.0));
        tap.consumer.advance(read + 256, 156, base + 9 * 256);
        assert_eq!(
            tap.snapshot().unwrap().residence_since(partial),
            Some(256.0)
        );
    }

    #[test]
    fn discarded_frames_and_unrecorded_production_skip_only_their_window() {
        let tap = Passage::default();
        tap.producer.advance(64, 64, 1_000);
        tap.consumer.advance(32, 32, 1_010);
        let first = tap.snapshot().unwrap();
        // Discard passes the remaining 32 queued frames without reading them.
        tap.consumer.advance(64, 0, 1_020);
        let discarded = tap.snapshot().unwrap();
        assert_eq!(discarded.residence_since(first), None);
        tap.producer.advance(96, 32, 1_030);
        tap.consumer.advance(96, 32, 1_050);
        assert_eq!(
            tap.snapshot().unwrap().residence_since(discarded),
            Some(20.0)
        );
        // Unchanged cursors record nothing, so empty polls are not passages.
        tap.consumer.advance(96, 0, 2_000);
        assert_eq!(tap.consumer.snapshot().unwrap().latest, 1_050);
        tap.consumer.advance(128, 32, 1_060);
        assert_eq!(tap.snapshot(), None);
        tap.producer.sequence.store(1, Ordering::SeqCst);
        assert_eq!(tap.producer.snapshot(), None);
    }

    /// A reporter descheduled between the two sides must not count frames consumed
    /// meanwhile as a backlog: this steady one-period queue would read 13696 ticks.
    #[test]
    fn consumer_progress_during_a_snapshot_skips_that_window() {
        let tap = Passage::default();
        let mut period = |index: u64| {
            tap.producer.advance((index + 1) * 256, 256, index * 256);
            if index != 0 {
                tap.consumer.advance(index * 256, 256, index * 256);
            }
        };
        (0..4).for_each(&mut period);
        let previous = tap.snapshot().unwrap();
        (4..8).for_each(&mut period);
        let stale = tap.consumer.snapshot().unwrap();
        (8..28).for_each(&mut period);
        let producer = tap.producer.snapshot().unwrap();
        let after = tap.consumer.snapshot().unwrap();
        assert_eq!(coherent(stale, producer, after), None);
        let torn = PassageSnapshot {
            producer,
            consumer: stale,
        };
        assert_eq!(torn.residence_since(previous), Some(13_696.0));
        let next = tap.snapshot().unwrap();
        assert_eq!(next.residence_since(previous), Some(256.0));
    }
}
