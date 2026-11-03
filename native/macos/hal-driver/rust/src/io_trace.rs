//! Opt-in, bounded HAL callback evidence without retaining audio samples.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::io_outcome::IoOutcome;

// The installed 128/512-frame client pair exceeded 4,096 rows before collection.
// Keep its early signal window as well; longer sessions report overwrites explicitly.
const CAPACITY: usize = 8192;
const FIELDS: usize = 10;
pub(crate) const HEADER_WORDS: usize = 5 + crate::io_admission::COUNTERS + 4;

pub(crate) struct IoTrace {
    pub(crate) admission: crate::io_admission::Admission,
    enabled: AtomicBool,
    next: AtomicU64,
    dropped: AtomicU64,
    rows: Box<[Row]>,
    clock: fn() -> u64,
}

pub(crate) struct CallbackStart(u64);

#[cfg(target_os = "macos")]
fn callback_clock() -> u64 {
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
    }
    // SAFETY: this non-blocking counter read has no pointer or lifecycle preconditions.
    unsafe { mach_absolute_time() }
}

#[cfg(not(target_os = "macos"))]
fn callback_clock() -> u64 {
    0
}

struct Row {
    sequence: AtomicU64,
    fields: [AtomicU64; FIELDS],
}

#[derive(Clone, Copy)]
pub(crate) struct Observation<'a> {
    pub render: bool,
    pub client: u32,
    pub frame: u64,
    pub channels: usize,
    pub samples: &'a [f32],
    pub outcome: IoOutcome,
}

impl IoTrace {
    pub(crate) fn new() -> Result<Self, std::collections::TryReserveError> {
        Self::with_clock(callback_clock)
    }

    fn with_clock(clock: fn() -> u64) -> Result<Self, std::collections::TryReserveError> {
        let mut rows = Vec::new();
        rows.try_reserve_exact(CAPACITY)?;
        rows.extend((0..CAPACITY).map(|_| Row {
            sequence: AtomicU64::new(0),
            fields: std::array::from_fn(|_| AtomicU64::new(0)),
        }));
        Ok(Self {
            admission: crate::io_admission::Admission::new(),
            enabled: AtomicBool::new(false),
            next: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            rows: rows.into_boxed_slice(),
            clock,
        })
    }

    /// Control holds the device freeze, excluding callbacks during reset.
    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.enabled.store(false, Ordering::Release);
        self.admission.reset();
        self.next.store(0, Ordering::Relaxed);
        self.dropped.store(0, Ordering::Relaxed);
        for row in &self.rows {
            row.sequence.store(0, Ordering::Relaxed);
        }
        // Release makes the reset visible to callbacks admitted after the freeze.
        self.enabled.store(enabled, Ordering::Release);
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Control retains the device lock and has reset while frozen. Publish after
    /// releasing that freeze so enabling evidence cannot itself count a frozen IO.
    pub(crate) fn enable_after_reset(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    pub(crate) fn begin(&self) -> Option<CallbackStart> {
        self.enabled().then(|| CallbackStart((self.clock)()))
    }

    pub(crate) fn note(&self, stage: crate::io_admission::Stage) {
        if self.enabled() {
            self.admission.note(stage);
        }
    }

    pub(crate) fn reject_extent(&self, frames: u32, buffer_present: bool, render: bool) {
        if self.enabled() {
            self.admission.extent(frames, buffer_present, render);
        }
    }

    pub(crate) fn reject_timestamp(&self, sample_time: f64, render: bool) {
        if self.enabled() {
            self.admission.note(if render {
                crate::io_admission::Stage::Write
            } else {
                crate::io_admission::Stage::Read
            });
            self.admission.timestamp(sample_time);
        }
    }

    pub(crate) fn record(&self, observation: Observation<'_>, start: Option<CallbackStart>) {
        let Some(start) = start else {
            return;
        };
        let end = (self.clock)();
        if !self.enabled() || observation.channels == 0 {
            return;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        if id > u64::MAX / 2 {
            // A snapshot acquiring this count must also see the reserved next ID.
            self.dropped.fetch_add(1, Ordering::Release);
            self.enabled.store(false, Ordering::Release);
            return;
        }
        let Ok(slot) = usize::try_from((id - 1) % CAPACITY as u64) else {
            return;
        };
        let Some(row) = self.rows.get(slot) else {
            return;
        };
        let previous = row.sequence.load(Ordering::SeqCst);
        // One bounded claim; concurrent readers/writers never wait for evidence.
        if previous & 1 != 0
            || previous >= id * 2
            || row
                .sequence
                .compare_exchange(previous, id * 2 - 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            self.dropped.fetch_add(1, Ordering::Release);
            return;
        }
        let mut zero_frames = 0;
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for frame in observation.samples.chunks_exact(observation.channels) {
            zero_frames += u64::from(frame.iter().all(|sample| *sample == 0.0));
            for sample in frame {
                hash = (hash ^ u64::from(sample.to_bits())).wrapping_mul(0x100_0000_01b3);
            }
        }
        let fields = [
            u64::from(observation.render),
            observation.frame,
            (observation.samples.len() / observation.channels) as u64,
            u64::from(observation.outcome.accepted),
            u64::from(observation.outcome.available),
            zero_frames,
            hash,
            u64::from(observation.client),
            start.0,
            end,
        ];
        // SeqCst cells make a snapshot's two sequence reads bracket every field;
        // an overwritten or still-publishing row is refused instead of mixed.
        for (cell, value) in row.fields.iter().zip(fields) {
            cell.store(value, Ordering::SeqCst);
        }
        row.sequence.store(id * 2, Ordering::SeqCst);
    }

    /// Fixed-size, native-endian `UInt64` payload: version, fields, capacity, issued,
    /// dropped, admission counters/rejected arguments, then (sequence, render, frame, frames, accepted, available, zeros,
    /// hash, client, `host_start`, `host_end`). Host counters bracket processing, excluding tracing.
    pub(crate) fn snapshot(&self) -> Vec<u8> {
        // Acquire drops first: each dropped reservation precedes the issued bound.
        let dropped = self.dropped.load(Ordering::Acquire);
        let issued = self.next.load(Ordering::Acquire);
        self.snapshot_at(issued, dropped)
    }

    fn snapshot_at(&self, issued: u64, dropped: u64) -> Vec<u8> {
        let mut words = Vec::with_capacity(HEADER_WORDS + CAPACITY * (FIELDS + 1));
        words.extend([5, FIELDS as u64, CAPACITY as u64, issued, dropped]);
        words.extend(self.admission.snapshot());
        for row in &self.rows {
            let before = row.sequence.load(Ordering::SeqCst);
            let fields = row
                .fields
                .each_ref()
                .map(|cell| cell.load(Ordering::SeqCst));
            let after = row.sequence.load(Ordering::SeqCst);
            // Callbacks continue during this control-thread copy. Later reservations
            // are outside its declared prefix, even when their row is already complete.
            if before != 0 && before & 1 == 0 && before == after && before / 2 <= issued {
                words.push(before / 2);
                words.extend(fields);
            } else {
                words.extend([0; FIELDS + 1]);
            }
        }
        words.into_iter().flat_map(u64::to_ne_bytes).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(trace: &IoTrace) -> Vec<u64> {
        trace
            .snapshot()
            .chunks_exact(size_of::<u64>())
            .map(|value| u64::from_ne_bytes(value.try_into().unwrap()))
            .collect()
    }

    #[test]
    fn a_callback_published_after_the_snapshot_bound_cannot_escape_the_declared_prefix() {
        let trace = IoTrace::with_clock(|| 1234).unwrap();
        trace.set_enabled(true);
        let observation = Observation {
            render: true,
            client: 42,
            frame: 0,
            channels: 1,
            samples: &[0.5],
            outcome: IoOutcome::completed(1),
        };
        trace.record(observation, trace.begin());
        let issued = trace.next.load(Ordering::Acquire);
        trace.record(
            Observation {
                frame: 1,
                ..observation
            },
            trace.begin(),
        );
        let bytes = trace.snapshot_at(issued, 0);
        let words: Vec<_> = bytes
            .chunks_exact(8)
            .map(|word| u64::from_ne_bytes(word.try_into().unwrap()))
            .collect();
        let sequences: Vec<_> = words[HEADER_WORDS..]
            .chunks_exact(FIELDS + 1)
            .filter_map(|row| (row[0] != 0).then_some(row[0]))
            .collect();
        assert_eq!(words[3], 1);
        assert_eq!(sequences, [1]);
    }

    #[test]
    fn disabled_and_enabled_callback_evidence_never_allocates() {
        let trace = IoTrace::with_clock(|| 1234).unwrap();
        let pcm = [0.25, -0.25, 0.0, 0.0];
        for enabled in [false, true] {
            trace.set_enabled(enabled);
            let measured = allocation_counter::measure(|| {
                trace.record(
                    Observation {
                        render: false,
                        client: 0,
                        frame: 64,
                        channels: 2,
                        samples: &pcm,
                        outcome: IoOutcome::completed(1),
                    },
                    trace.begin(),
                );
            });
            assert_eq!(measured.count_total, 0);
            let actual = words(&trace);
            assert_eq!(actual[3], u64::from(enabled));
            if enabled {
                assert_eq!(
                    &actual[HEADER_WORDS..HEADER_WORDS + 7],
                    &[1, 0, 64, 2, 1, 1, 1]
                );
                assert_eq!(&actual[HEADER_WORDS + 9..HEADER_WORDS + 11], &[1234, 1234]);
            }
        }
    }

    #[test]
    fn bounded_history_marks_overwrites_and_reset_removes_old_observations() {
        let trace = IoTrace::new().unwrap();
        trace.set_enabled(true);
        for frame in 0..CAPACITY as u64 + 3 {
            trace.record(
                Observation {
                    render: true,
                    client: 42,
                    frame,
                    channels: 1,
                    samples: &[0.5],
                    outcome: IoOutcome::completed(1),
                },
                trace.begin(),
            );
        }
        let actual = words(&trace);
        assert_eq!(actual[3], CAPACITY as u64 + 3);
        assert_eq!(actual[4], 0);
        let mut sequences: Vec<_> = actual[HEADER_WORDS..]
            .chunks_exact(FIELDS + 1)
            .map(|row| row[0])
            .collect();
        sequences.sort_unstable();
        assert_eq!(sequences, (4..=CAPACITY as u64 + 3).collect::<Vec<_>>());
        trace.set_enabled(false);
        assert!(words(&trace)[3..].iter().all(|word| *word == 0));
    }
}
