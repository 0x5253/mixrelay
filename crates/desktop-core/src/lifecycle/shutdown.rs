//! Quit is admitted once, held while the engine tears down, and reopened if teardown fails.
use std::sync::atomic::{AtomicBool, Ordering};

/// What the application does with one exit request.
#[derive(Debug, PartialEq, Eq)]
pub enum ExitRequest {
    /// Teardown was confirmed; the process may exit.
    Allow,
    /// Hold the process; an earlier request is still tearing down.
    Hold,
    /// Hold the process and start teardown.
    Begin,
}

/// Shutdown state of one controller, shared by its exit handler and engine watchers.
#[derive(Debug, Default)]
pub struct ShutdownGate {
    requested: AtomicBool,
    stopping: AtomicBool,
    finished: AtomicBool,
}

impl ShutdownGate {
    pub fn request_exit(&self) -> ExitRequest {
        if self.finished.load(Ordering::Acquire) {
            return ExitRequest::Allow;
        }
        self.requested.store(true, Ordering::Release);
        if self.stopping.swap(true, Ordering::AcqRel) {
            ExitRequest::Hold
        } else {
            ExitRequest::Begin
        }
    }

    /// Teardown failed; the next exit request begins it again.
    pub fn attempt_failed(&self) {
        self.stopping.store(false, Ordering::Release);
    }

    /// Teardown is confirmed; every later exit request is allowed.
    pub fn complete(&self) {
        self.finished.store(true, Ordering::Release);
    }

    /// A stop has been requested at least once. Unlike [`Self::stopping`] this survives a
    /// failed attempt, so work queued before the first request never revives the engine.
    pub fn requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    /// Teardown is running or has completed.
    pub fn stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests;
