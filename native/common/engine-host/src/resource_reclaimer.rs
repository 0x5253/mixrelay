//! Joined control-thread ownership for retiring platform audio resources.

use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// Keeps reclamation alive during audio processing and joins it during shutdown.
///
/// Create this owner before the audio-worker guard, so reverse drop order stops
/// audio first. The callback runs only on this control thread, never in a realtime
/// callback. Dropping this guard waits for any in-flight native teardown; the
/// polling interval is not a hard shutdown deadline for native driver calls.
pub struct ResourceReclaimer {
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl ResourceReclaimer {
    /// Starts a named reclamation thread, polling at the existing 100 ms cadence.
    /// The callback returns false after its weak resource owners expire.
    ///
    /// # Errors
    /// Returns the OS thread-creation error without detaching any worker.
    pub fn start(
        name: &str,
        mut reclaim: impl FnMut() -> bool + Send + 'static,
    ) -> io::Result<Self> {
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);
        let worker = thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                while !worker_stopping.load(Ordering::Acquire) && reclaim() {
                    thread::park_timeout(Duration::from_millis(100));
                }
            })?;
        Ok(Self {
            stopping,
            worker: Some(worker),
        })
    }
}

impl ResourceReclaimer {
    /// Stops and joins the worker.
    ///
    /// # Errors
    /// Reports a reclaimer that panicked, so the service does not count as cleanly stopped.
    pub fn finish(mut self) -> Result<(), String> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> Result<(), String> {
        self.stopping.store(true, Ordering::Release);
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        worker.thread().unpark();
        worker
            .join()
            .map_err(|_| "audio resource reclaimer panicked".to_owned())
    }
}

impl Drop for ResourceReclaimer {
    fn drop(&mut self) {
        if self.stop_and_join().is_err() {
            tracing::error!("audio resource reclaimer panicked during shutdown");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn drop_waits_for_in_flight_reclamation_and_releases_its_owner() {
        let owner = Arc::new(());
        let weak = Arc::downgrade(&owner);
        let (entered, entry) = mpsc::sync_channel(1);
        let (release, released) = mpsc::sync_channel(1);
        let reclaimer = ResourceReclaimer::start("reclaimer-test", move || {
            entered.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(Arc::strong_count(&owner), 1);
            false
        })
        .unwrap();
        entry.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(weak.upgrade().is_some());
        let (dropping, drop_started) = mpsc::sync_channel(1);
        let (finished, completion) = mpsc::sync_channel(1);
        let shutdown = thread::spawn(move || {
            dropping.send(()).unwrap();
            drop(reclaimer);
            finished.send(()).unwrap();
        });
        drop_started.recv_timeout(Duration::from_secs(5)).unwrap();
        let waiting = completion.recv_timeout(Duration::from_millis(100));
        release.send(()).unwrap();
        shutdown.join().unwrap();
        assert_eq!(waiting, Err(mpsc::RecvTimeoutError::Timeout));
        completion.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn false_callback_ends_worker_while_guard_remains_alive() {
        let (finished, receiver) = mpsc::sync_channel(1);
        let reclaimer = ResourceReclaimer::start("reclaimer-ended-test", move || {
            finished.send(()).unwrap();
            false
        })
        .unwrap();
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        // Once the callback exits, its only sender is dropped; this proves the
        // worker finished without depending on timing or dropping the guard.
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        drop(reclaimer);
    }

    #[test]
    fn stop_joins_even_while_resource_owners_remain_live() {
        let owner = Arc::new(());
        let worker_owner = Arc::clone(&owner);
        let (started, receiver) = mpsc::sync_channel(1);
        let mut first = true;
        let reclaimer = ResourceReclaimer::start("reclaimer-stop-test", move || {
            assert!(Arc::strong_count(&worker_owner) >= 2);
            if first {
                started.send(()).unwrap();
                first = false;
            }
            true
        })
        .unwrap();
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(reclaimer);
        assert_eq!(Arc::strong_count(&owner), 1);
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }

    #[test]
    fn finish_reports_a_reclaimer_that_panicked() {
        let quiet = ResourceReclaimer::start("mixrelay-test-reclaimer", || false).unwrap();
        assert_eq!(quiet.finish(), Ok(()));
        let panicking =
            ResourceReclaimer::start("mixrelay-test-reclaimer", || panic!("reclaim failed"))
                .unwrap();
        while !panicking.worker.as_ref().unwrap().is_finished() {
            thread::yield_now();
        }
        assert_eq!(
            panicking.finish(),
            Err("audio resource reclaimer panicked".to_owned())
        );
        // Dropped rather than finished, the panic is logged instead of returned.
        let dropped =
            ResourceReclaimer::start("mixrelay-test-reclaimer", || panic!("dropped")).unwrap();
        while !dropped.worker.as_ref().unwrap().is_finished() {
            thread::yield_now();
        }
        drop(dropped);
    }
}
