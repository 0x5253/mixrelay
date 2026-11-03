use super::*;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

struct CaptureQueue {
    samples: RefCell<VecDeque<f32>>,
    capacity: usize,
    overflows: Cell<u64>,
    read: Cell<u64>,
}
struct QueueStats {
    overflows: u64,
    queued: u64,
    read: u64,
}
impl CaptureQueue {
    fn new(frames: usize) -> Self {
        Self {
            samples: RefCell::new(VecDeque::with_capacity(frames)),
            capacity: frames,
            overflows: Cell::new(0),
            read: Cell::new(0),
        }
    }
    fn stats(&self) -> QueueStats {
        QueueStats {
            overflows: self.overflows.get(),
            queued: self.samples.borrow().len() as u64,
            read: self.read.get(),
        }
    }
}
fn write(ring: &CaptureQueue, samples: &[f32]) -> u32 {
    let mut queue = ring.samples.borrow_mut();
    if samples.len() > ring.capacity - queue.len() {
        ring.overflows.set(ring.overflows.get() + 1);
        return 0;
    }
    queue.extend(samples.iter().copied());
    u32::try_from(samples.len()).unwrap()
}
fn read(ring: &CaptureQueue, destination: &mut [f32]) -> u32 {
    let mut queue = ring.samples.borrow_mut();
    let frames = destination.len().min(queue.len());
    destination.fill(0.0);
    for sample in &mut destination[..frames] {
        *sample = queue.pop_front().unwrap();
    }
    ring.read.set(ring.read.get() + frames as u64);
    u32::try_from(frames).unwrap()
}

#[test]
fn rejected_native_input_invalidates_full_delivery_once_without_discarding_valid_pcm() {
    let ring = CaptureQueue::new(8);
    assert_eq!(write(&ring, &[1.0, 2.0, 3.0, 4.0]), 4);
    assert_eq!(write(&ring, &[5.0, 6.0, 7.0, 8.0]), 4);
    assert_eq!(write(&ring, &[9.0, 10.0, 11.0, 12.0]), 0);
    let mut cursor = CaptureRead::default();
    let mut output = [0.0; 4];
    let outcome = cursor.read(
        &mut output,
        1,
        || ring.stats().overflows,
        |samples| read(&ring, samples),
    );
    assert_eq!(outcome.frames, 4);
    assert!(outcome.discontinuity);
    assert_eq!(
        output.map(f32::to_bits),
        [1.0_f32, 2.0, 3.0, 4.0].map(f32::to_bits)
    );
    let outcome = cursor.read(
        &mut output,
        1,
        || ring.stats().overflows,
        |samples| read(&ring, samples),
    );
    assert_eq!(outcome.frames, 4);
    assert!(!outcome.discontinuity);
    assert_eq!(
        output.map(f32::to_bits),
        [5.0_f32, 6.0, 7.0, 8.0].map(f32::to_bits)
    );
    assert_eq!(ring.stats().read, 8);
}

#[test]
fn rejection_during_read_is_reported_without_waiting_for_diagnostic_polling() {
    let failures = Cell::new(0);
    let mut cursor = CaptureRead::default();
    let mut output = [0.0; 2];
    let outcome = cursor.read(
        &mut output,
        1,
        || failures.get(),
        |samples| {
            samples.copy_from_slice(&[0.25, 0.5]);
            failures.set(1);
            2
        },
    );
    assert_eq!(outcome.frames, 2);
    assert!(outcome.discontinuity);
    assert_eq!(output.map(f32::to_bits), [0.25_f32, 0.5].map(f32::to_bits));
    let next = cursor.read(&mut output, 1, || failures.get(), |_| 0);
    assert!(!next.discontinuity);
}

#[test]
fn another_streams_failure_does_not_invalidate_this_source() {
    let mut affected = CaptureRead::default();
    let mut unaffected = CaptureRead::default();
    let mut output = [0.0; 1];
    assert!(affected.read(&mut output, 1, || 1, |_| 1).discontinuity);
    assert!(!unaffected.read(&mut output, 1, || 0, |_| 1).discontinuity);
    assert!(!affected.read(&mut output, 1, || 1, |_| 1).discontinuity);
}

#[test]
fn failure_count_wrap_and_an_empty_read_still_publish_new_source_loss() {
    let mut cursor = CaptureRead {
        last_failure: u64::MAX,
        ..CaptureRead::default()
    };
    let outcome = cursor.read(&mut [], 1, || 0, |_| 0);
    assert_eq!(outcome.frames, 0);
    assert!(outcome.discontinuity);
    assert!(!cursor.read(&mut [], 1, || 0, |_| 0).discontinuity);
}

fn discard(ring: &CaptureQueue) {
    ring.samples.borrow_mut().clear();
}

fn read_guarded(
    cursor: &mut CaptureRead,
    ring: &CaptureQueue,
    output: &mut [f32],
) -> TransferOutcome {
    cursor.prepare_read(|| discard(ring));
    cursor.read(
        output,
        1,
        || ring.stats().overflows,
        |samples| read(ring, samples),
    )
}

#[test]
fn unused_source_discards_off_pcm_including_frames_before_the_first_resumed_read() {
    let ring = CaptureQueue::new(8);
    let mut cursor = CaptureRead::default();
    let mut output = [0.0; 4];
    assert_eq!(write(&ring, &[1.0; 4]), 4);
    assert_eq!(write(&ring, &[1.0; 4]), 4);
    assert_eq!(read_guarded(&mut cursor, &ring, &mut output).frames, 4);
    cursor.finish_block(|| discard(&ring));
    assert_eq!(ring.stats().queued, 4, "read owner retains its queue");

    assert_eq!(write(&ring, &[2.0; 4]), 4);
    let allocation = allocation_counter::measure(|| {
        cursor.finish_block(|| discard(&ring));
    });
    assert_eq!(allocation.count_total, 0);
    assert_eq!(allocation.count_current, 0);
    assert_eq!(
        ring.stats().queued,
        0,
        "the OFF block releases its queued PCM"
    );
    assert_eq!(write(&ring, &[3.0; 4]), 4);
    let mut resumed = TransferOutcome::complete(u32::MAX);
    let allocation = allocation_counter::measure(|| {
        resumed = read_guarded(&mut cursor, &ring, &mut output);
    });
    assert_eq!(allocation.count_total, 0);
    assert_eq!(allocation.count_current, 0);
    assert_eq!(
        resumed.frames, 0,
        "late OFF-period PCM cannot be delivered on ON"
    );
    assert!(output.iter().all(|sample| sample.to_bits() == 0));
    cursor.finish_block(|| discard(&ring));

    assert_eq!(write(&ring, &[4.0; 4]), 4);
    assert_eq!(write(&ring, &[4.0; 4]), 4);
    assert_eq!(read_guarded(&mut cursor, &ring, &mut output).frames, 4);
    assert_eq!(output.map(f32::to_bits), [4.0_f32; 4].map(f32::to_bits));
}

#[test]
fn shared_source_with_any_read_and_an_unrelated_source_keep_their_own_pcm() {
    let shared = CaptureQueue::new(8);
    let unrelated = CaptureQueue::new(8);
    let mut shared_cursor = CaptureRead::default();
    let mut unrelated_cursor = CaptureRead::default();
    for (ring, sample) in [(&shared, 0.25), (&unrelated, 0.5)] {
        assert_eq!(write(ring, &[sample; 4]), 4);
        assert_eq!(write(ring, &[sample; 4]), 4);
    }
    let mut output = [0.0; 2];
    // Two graph bindings use the same canonical source; neither may cause the
    // block-end idle drain while their owner was read at least once.
    assert_eq!(
        read_guarded(&mut shared_cursor, &shared, &mut output).frames,
        2
    );
    assert_eq!(
        read_guarded(&mut shared_cursor, &shared, &mut output).frames,
        2
    );
    assert_eq!(
        read_guarded(&mut unrelated_cursor, &unrelated, &mut output).frames,
        2
    );
    let allocation = allocation_counter::measure(|| {
        shared_cursor.finish_block(|| discard(&shared));
        unrelated_cursor.finish_block(|| discard(&unrelated));
    });
    assert_eq!(allocation.count_total, 0);
    assert_eq!(allocation.count_current, 0);
    assert_eq!(shared.stats().queued, 4);
    assert_eq!(unrelated.stats().queued, 6);
    shared_cursor.finish_block(|| discard(&shared));
    assert_eq!(shared.stats().queued, 0);
    assert_eq!(
        read_guarded(&mut unrelated_cursor, &unrelated, &mut output).frames,
        2
    );
    assert_eq!(output.map(f32::to_bits), [0.5_f32; 2].map(f32::to_bits));
}

#[test]
fn partial_source_read_replaces_only_admitted_samples_and_reports_them_once() {
    let mut cursor = CaptureRead::default();
    let mut output = [0.0; 8];
    let finite = f32::from_bits(1);
    let outcome = cursor.read(
        &mut output,
        2,
        || 0,
        |samples| {
            samples.copy_from_slice(&[
                f32::NAN,
                -0.0,
                f32::INFINITY,
                finite,
                f32::NEG_INFINITY,
                0.25,
                f32::NAN,
                0.5,
            ]);
            3
        },
    );
    assert_eq!(outcome.frames, 3);
    assert_eq!(outcome.source_sample_faults, 3);
    assert!(!outcome.discontinuity);
    assert_eq!(
        output[..6]
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>(),
        [0.0_f32, -0.0, 0.0, finite, 0.0, 0.25].map(f32::to_bits),
    );
    assert!(
        output[6].is_nan(),
        "unadmitted tail is owned by the graph clear"
    );
    let empty = cursor.read(&mut output, 2, || 0, |_| 0);
    assert_eq!(empty.source_sample_faults, 0);
}
