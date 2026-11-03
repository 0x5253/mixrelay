//! Processing-owner lifetime and block-boundary suspension, independent of device policy.

use std::{
    sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkerState {
    NotStarted,
    Running,
    Stopped,
    Failed,
}

#[derive(Debug, Default)]
pub(super) struct ProcessingLifecycle {
    stop_requested: AtomicBool,
    worker_state: AtomicU32,
    suspension: Suspension,
}

impl ProcessingLifecycle {
    pub(super) fn begin(&self) -> Result<ProcessingLease<'_>, &'static str> {
        self.worker_state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "session processing already started")?;
        Ok(ProcessingLease { lifecycle: self })
    }

    pub(super) fn state(&self) -> WorkerState {
        match self.worker_state.load(Ordering::Acquire) {
            0 => WorkerState::NotStarted,
            1 => WorkerState::Running,
            2 => WorkerState::Stopped,
            3 => WorkerState::Failed,
            _ => unreachable!("only lifecycle transitions publish worker state"),
        }
    }

    pub(super) fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::Release);
    }

    pub(super) fn stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::Acquire)
    }

    pub(super) fn checkpoint(&self) -> bool {
        let ticket = self.suspension.requested.load(Ordering::Acquire);
        self.suspension
            .acknowledged
            .store(ticket, Ordering::Release);
        ticket != 0
    }

    pub(super) fn pause_requested(&self) -> bool {
        self.suspension.requested.load(Ordering::Acquire) != 0
    }

    // The session owns failure/rollback policy and decides whether cancellation
    // is permitted. This boundary only implements the suspension handshake.
    pub(super) fn request_pause(&self) -> u64 {
        let ticket = self.suspension.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        self.suspension.requested.store(ticket, Ordering::Release);
        ticket
    }

    pub(super) fn await_pause(
        &self,
        ticket: u64,
        timeout: Duration,
        time: &dyn super::ProcessingTime,
    ) -> bool {
        let deadline = mixrelay_time::Deadline::after(time.now(), timeout);
        while self.suspension.acknowledged.load(Ordering::Acquire) != ticket {
            if deadline.has_passed(time.now()) {
                return false;
            }
            time.sleep_until(mixrelay_time::Deadline::after(
                time.now(),
                Duration::from_millis(1),
            ));
        }
        true
    }

    pub(super) fn resume(&self) {
        self.suspension.requested.store(0, Ordering::Release);
    }
}

#[derive(Debug, Default)]
struct Suspension {
    requested: AtomicU64,
    sequence: AtomicU64,
    acknowledged: AtomicU64,
}

/// Sole processing-owner lease. It borrows lifecycle state, not device resources
/// or format policy. Dropping after cancellation publishes `Stopped`; an early
/// return or unwind publishes `Failed`. Restart requires a fresh session.
#[derive(Debug)]
pub struct ProcessingLease<'a> {
    lifecycle: &'a ProcessingLifecycle,
}

impl Drop for ProcessingLease<'_> {
    fn drop(&mut self) {
        let state = if self.lifecycle.stop_requested() && !std::thread::panicking() {
            2
        } else {
            3
        };
        self.lifecycle.worker_state.store(state, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
