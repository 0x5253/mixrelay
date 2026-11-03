//! Native clients and recovery stay on one owning worker. A graph edit is not
//! required to reopen an invalidated device, and removing the owner stops retries.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
    },
    time::Duration,
};

/// A native attempt calls `connected` only after starting successfully. Initial
/// failure rejects graph preparation; failures after activation retry without
/// changing the graph. `attempt` must release its native client before returning.
pub fn supervise(
    stop: &AtomicBool,
    failed: &AtomicBool,
    ready: &SyncSender<Result<(), String>>,
    mut attempt: impl FnMut(&mut dyn FnMut()) -> Result<(), String>,
) {
    let mut started = false;
    while !stop.load(Ordering::Acquire) {
        let result = attempt(&mut || {
            started = true;
            failed.store(false, Ordering::Release);
            let _ = ready.try_send(Ok(()));
        });
        failed.store(true, Ordering::Release);
        if !started {
            let _ = ready.try_send(Err(result
                .err()
                .unwrap_or_else(|| "device startup cancelled".into())));
            return;
        }
        if !stop.load(Ordering::Acquire) {
            // Drop unparks this owning worker, so shutdown does not wait out the
            // retry interval. No detached polling or native COM/CoreAudio owner.
            std::thread::park_timeout(Duration::from_millis(500));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_failure_is_not_silently_accepted() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let stop = AtomicBool::new(false);
        let failed = AtomicBool::new(false);
        let mut attempts = 0;
        supervise(&stop, &failed, &tx, |_| {
            attempts += 1;
            Err("absent".into())
        });
        assert_eq!(attempts, 1);
        assert_eq!(rx.recv().unwrap(), Err("absent".into()));
        assert!(failed.load(Ordering::Acquire));
    }
    #[test]
    fn stop_during_successful_attempt_prevents_reopening() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let stop = AtomicBool::new(false);
        let failed = AtomicBool::new(true);
        let mut attempts = 0;
        supervise(&stop, &failed, &tx, |connected| {
            attempts += 1;
            connected();
            assert!(!failed.load(Ordering::Acquire));
            stop.store(true, Ordering::Release);
            Err("removed".into())
        });
        assert_eq!(attempts, 1);
        assert_eq!(rx.recv().unwrap(), Ok(()));
    }
    #[test]
    fn reconnect_clears_fault_and_retains_one_worker() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let stop = AtomicBool::new(false);
        let failed = AtomicBool::new(true);
        let mut attempts = 0;
        supervise(&stop, &failed, &tx, |connected| {
            attempts += 1;
            if attempts == 2 {
                assert!(failed.load(Ordering::Acquire));
            }
            connected();
            assert!(!failed.load(Ordering::Acquire));
            if attempts == 2 {
                stop.store(true, Ordering::Release);
            }
            // Avoid sleeping in this deterministic state/lifetime test.
            std::thread::current().unpark();
            Err("invalidated".into())
        });
        assert_eq!(attempts, 2);
        assert_eq!(rx.recv().unwrap(), Ok(()));
    }
}
