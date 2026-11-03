use super::*;
use crate::capture::ownership::CaptureHandoff;
use std::sync::atomic::AtomicUsize;

struct Capture {
    claim: AtomicBool,
    drops: Arc<AtomicUsize>,
}

impl ReaderOwner for Capture {
    fn reader_claim(&self) -> &AtomicBool {
        &self.claim
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

fn capture() -> (Arc<Capture>, Arc<AtomicUsize>) {
    let drops = Arc::new(AtomicUsize::new(0));
    (
        Arc::new(Capture {
            claim: AtomicBool::new(false),
            drops: Arc::clone(&drops),
        }),
        drops,
    )
}

#[test]
fn second_reader_is_rejected_and_aborted_preparation_releases_claim() {
    let (capture, drops) = capture();
    let prepared = ExclusiveReader::acquire(Arc::clone(&capture)).unwrap();
    assert!(capture.claim.load(Ordering::Acquire));
    assert!(ExclusiveReader::acquire(Arc::clone(&capture)).is_none());
    drop(prepared);
    assert!(!capture.claim.load(Ordering::Acquire));
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    let retry = ExclusiveReader::acquire(Arc::clone(&capture)).unwrap();
    drop(retry);
    drop(capture);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn reader_keeps_native_owner_alive_after_telemetry_owner_is_removed() {
    let (capture, drops) = capture();
    let reader = ExclusiveReader::acquire(Arc::clone(&capture)).unwrap();
    drop(capture);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    assert!(reader.owner().claim.load(Ordering::Acquire));
    drop(reader);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn prepared_reader_does_not_replace_active_consumer_until_committed_handoff() {
    let (old, old_drops) = capture();
    let (next, next_drops) = capture();
    let mut worker = ExclusiveReader::acquire(Arc::clone(&old)).unwrap();
    let prepared = ExclusiveReader::acquire(Arc::clone(&next)).unwrap();
    let mailbox = CaptureHandoff::default();
    assert!(mailbox.try_take().is_none());
    assert!(std::ptr::eq(worker.owner(), old.as_ref()));
    // HTTP acceptance moves, never clones or reacquires, the reserved reader.
    mailbox.publish(prepared);
    assert!(ExclusiveReader::acquire(Arc::clone(&old)).is_none());
    assert!(ExclusiveReader::acquire(Arc::clone(&next)).is_none());
    worker = mailbox.try_take().unwrap();
    assert!(!old.claim.load(Ordering::Acquire));
    assert!(std::ptr::eq(worker.owner(), next.as_ref()));
    assert!(next.claim.load(Ordering::Acquire));
    drop(worker);
    assert!(!next.claim.load(Ordering::Acquire));
    drop((old, next));
    assert_eq!(old_drops.load(Ordering::Relaxed), 1);
    assert_eq!(next_drops.load(Ordering::Relaxed), 1);
}

#[test]
fn rejected_replacement_keeps_active_claim_and_disposes_only_new_reader() {
    let (old, _) = capture();
    let (next, next_drops) = capture();
    let worker = ExclusiveReader::acquire(Arc::clone(&old)).unwrap();
    let prepared = ExclusiveReader::acquire(Arc::clone(&next)).unwrap();
    drop(next);
    drop(prepared);
    assert_eq!(next_drops.load(Ordering::Relaxed), 1);
    assert!(old.claim.load(Ordering::Acquire));
    assert!(ExclusiveReader::acquire(Arc::clone(&old)).is_none());
    drop(worker);
    assert!(!old.claim.load(Ordering::Acquire));
}

#[test]
fn concurrent_acquisition_grants_exactly_one_reader() {
    let (capture, _) = capture();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let owner = Arc::clone(&capture);
            let start = Arc::clone(&barrier);
            std::thread::spawn(move || {
                start.wait();
                ExclusiveReader::acquire(owner)
            })
        })
        .collect();
    let readers: Vec<_> = workers
        .into_iter()
        .filter_map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(readers.len(), 1);
    assert!(capture.claim.load(Ordering::Acquire));
    drop(readers);
    assert!(!capture.claim.load(Ordering::Acquire));
}
