//! Owns graph-path diagnostic delivery and its reporting thread.
use std::{
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use super::Window;

/// Publishes one diagnostic window with `try_lock`; the worker never waits.
pub(super) struct Reporter {
    slot: Arc<Mutex<Pending>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Pending {
    scheduled: Option<Window>,
    final_window: Option<Window>,
}

impl Reporter {
    pub(super) fn start(report: impl Fn(&Window) + Send + 'static) -> io::Result<Self> {
        let slot = Arc::new(Mutex::new(Pending::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (shared, stopping) = (Arc::clone(&slot), Arc::clone(&stop));
        let thread = std::thread::Builder::new()
            .name("mixrelay-worker-report".into())
            .spawn(move || {
                loop {
                    std::thread::park_timeout(Duration::from_secs(1));
                    let (pending, finished) = {
                        let mut slot = shared
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        // Read stop while holding the slot so a final publication cannot
                        // land between draining its slot and deciding to leave.
                        let finished = stopping.load(Ordering::Acquire);
                        (std::mem::take(&mut *slot), finished)
                    };
                    for window in [pending.scheduled, pending.final_window].iter().flatten() {
                        report(window);
                    }
                    if finished {
                        break;
                    }
                }
            })?;
        Ok(Self {
            slot,
            stop,
            thread: Some(thread),
        })
    }

    pub(super) fn publish(&self, window: Window) {
        if let Ok(mut slot) = self.slot.try_lock() {
            slot.scheduled = Some(window);
        }
    }

    /// Delivers the last partial window after the worker leaves realtime scheduling.
    pub(super) fn finish(self, window: Window) {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .final_window = Some(window);
        // Drop closes admission, wakes the reporter and joins after both slots drain.
    }
}

impl Drop for Reporter {
    fn drop(&mut self) {
        // Release publishes the final slot before the reporter observes shutdown.
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            if thread.join().is_err() {
                tracing::warn!("graph worker reporter failed while shutting down");
            }
        }
    }
}

pub(super) fn log(window: &Window) {
    let Window {
        counts,
        latency,
        realtime,
    } = window;
    tracing::info!(
        direct = counts.direct,
        hardware_blocks = counts.hardware,
        shared_source_blocks = counts.shared_source,
        timer_blocks = counts.timer,
        idle_waits = counts.idle_waits,
        wait_timeouts = counts.timeouts,
        time_read_unavailable = counts.time_reads.unavailable,
        time_read_not_ready = counts.time_reads.not_ready,
        time_read_ahead = counts.time_reads.ahead,
        time_read_behind = counts.time_reads.behind,
        time_read_mismatch = counts.time_reads.mismatch,
        time_read_incomplete = counts.time_reads.incomplete,
        capture_reacquisitions = counts.time_reads.reacquisitions,
        wake_p50_us = latency.map(|summary| summary.p50_us),
        wake_p99_us = latency.map(|summary| summary.p99_us),
        wake_max_us = latency.map(|summary| summary.maximum_us),
        realtime,
        "graph worker paths"
    );
}
