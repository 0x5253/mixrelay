//! Parent-process lifetime and creator-thread ownership of the worker's DSP instance.

use std::{
    io::Read,
    marker::PhantomData,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

/// Watch the worker's private stdin lifetime pipe before entering any vendor code.
///
/// The parent retains the write end until it releases this worker or exits.
/// EOF requests orderly shutdown, then terminates this isolated worker process if
/// vendor initialization, processing, or destruction has not returned within two seconds.
/// Returns an error if the watcher thread cannot be created.
pub(crate) fn watch_parent(shutdown: Arc<AtomicBool>) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("mixrelay-plugin-parent-watch".to_owned())
        .spawn(move || {
            let mut input = std::io::stdin();
            wait_for_parent_disconnect(&mut input);
            shutdown.store(true, Ordering::Release);
            // A native call may never observe shutdown. The process boundary owns its resources;
            // do not unwind or destroy the instance while its native code is still running.
            std::thread::sleep(std::time::Duration::from_secs(2));
            terminate_worker();
        })
}

fn wait_for_parent_disconnect(input: &mut impl Read) {
    let mut byte = [0_u8; 1];
    loop {
        match input.read(&mut byte) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

#[cfg(unix)]
fn terminate_worker() -> ! {
    // SAFETY: this process is the isolated worker whose sole parent pipe has closed. _exit
    // takes no borrowed memory and avoids arbitrary vendor exit handlers or concurrent drops.
    unsafe { libc::_exit(1) }
}

/// Failure to join the worker DSP thread because it panicked.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum DspThreadError {
    /// The child thread unwound before returning normally.
    #[error("processor DSP thread panicked")]
    ChildPanicked,
}

/// Creator-thread guard that keeps the native processor alive through DSP thread termination.
///
/// This type is deliberately `!Send` and `!Sync`: its owner must remain on the thread that
/// created and controls the native processor. Shutdown never abandons a possibly-running child;
/// process isolation remains the boundary for a stuck native plug-in.
pub(crate) struct DspThreadOwner<T> {
    handle: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    owner: Option<Arc<T>>,
    _creator_thread: PhantomData<Rc<()>>,
}

impl<T> DspThreadOwner<T> {
    /// Bind the running child and its native owner to the current creator thread.
    pub(crate) fn new(handle: JoinHandle<()>, shutdown: Arc<AtomicBool>, owner: Arc<T>) -> Self {
        Self {
            handle: Some(handle),
            shutdown,
            owner: Some(owner),
            _creator_thread: PhantomData,
        }
    }

    /// Signal shutdown and join the child exactly once.
    ///
    /// Repeated calls after the handle has been consumed succeed. The strong owner is released
    /// only after join completes, on the creator thread.
    ///
    /// # Errors
    /// Returns `DspThreadError::ChildPanicked` if the child unwound.
    pub(crate) fn stop_and_join(&mut self) -> Result<(), DspThreadError> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        self.shutdown.store(true, Ordering::Release);
        let result = handle.join().map_err(|_| DspThreadError::ChildPanicked);
        drop(self.owner.take());
        result
    }

    /// Join the child and combine its outcome with the broker's completion result.
    ///
    /// If both operations fail, both messages are preserved in broker-then-DSP order,
    /// separated by `; `.
    ///
    /// # Errors
    /// Returns the broker error, the DSP panic error, or both messages after joining.
    pub(crate) fn finish(mut self, broker_result: Result<(), String>) -> Result<(), String> {
        let dsp_result = self.stop_and_join().map_err(|error| error.to_string());
        match (broker_result, dsp_result) {
            (Ok(()), outcome) | (outcome, Ok(())) => outcome,
            (Err(broker), Err(dsp)) => Err(format!("{broker}; {dsp}")),
        }
    }
}

impl<T> Drop for DspThreadOwner<T> {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        panic::{AssertUnwindSafe, catch_unwind},
        sync::{Mutex, mpsc},
        thread,
        time::{Duration, Instant},
    };

    const CHILD_DEADLINE: Duration = Duration::from_millis(500);
    const TEST_DEADLINE: Duration = Duration::from_secs(2);

    #[test]
    fn interrupted_parent_read_is_not_a_disconnect() {
        struct InterruptedInput(usize);
        impl Read for InterruptedInput {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                self.0 += 1;
                match self.0 {
                    1 => Err(std::io::ErrorKind::Interrupted.into()),
                    2 => {
                        bytes[0] = 1;
                        Ok(1)
                    }
                    _ => Ok(0),
                }
            }
        }
        let mut input = InterruptedInput(0);
        wait_for_parent_disconnect(&mut input);
        assert_eq!(input.0, 3);
    }

    #[test]
    fn parent_disconnect_terminates_stuck_worker() {
        const CHILD_MODE: &str = "MIXRELAY_TEST_STUCK_PARENT_WATCH";
        if std::env::var_os(CHILD_MODE).is_some() {
            #[cfg(unix)]
            {
                extern "C" fn blocked_vendor_exit() {
                    loop {
                        thread::park();
                    }
                }
                // SAFETY: a static C-ABI callback with no captures, registered only in the
                // isolated child. The parent kills/reaps this child if termination fails.
                assert_eq!(unsafe { libc::atexit(blocked_vendor_exit) }, 0);
            }
            let _watch = watch_parent(Arc::new(AtomicBool::new(false))).unwrap();
            // A vendor call cannot be cooperatively interrupted by the shutdown flag.
            loop {
                thread::park();
            }
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "dsp_lifecycle::tests::parent_disconnect_terminates_stuck_worker",
                "--nocapture",
            ])
            .env(CHILD_MODE, "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_millis(100));
        assert!(child.try_wait().unwrap().is_none());
        drop(child.stdin.take());
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Err(_) => break None,
                Ok(None) if started.elapsed() >= Duration::from_secs(5) => break None,
                Ok(None) => thread::sleep(Duration::from_millis(10)),
            }
        };
        if status.is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
        assert_eq!(status.and_then(|status| status.code()), Some(1));
        eprintln!(
            "parent disconnect reaped stuck worker in {:?}",
            started.elapsed()
        );
    }

    #[derive(Debug, PartialEq, Eq)]
    struct OwnerDropRecord {
        thread_id: thread::ThreadId,
        child_finished: bool,
    }

    struct DropEvidence {
        child_finished: Arc<AtomicBool>,
        owner_drop: Mutex<Option<OwnerDropRecord>>,
    }

    struct DropProbe {
        evidence: Arc<DropEvidence>,
    }

    struct PoisonedBrokerRun {
        creator: thread::JoinHandle<()>,
        creator_id: mpsc::Receiver<thread::ThreadId>,
        child_observed_poison: mpsc::Receiver<bool>,
        creator_finished: mpsc::Receiver<bool>,
        release_child: mpsc::SyncSender<()>,
    }

    impl Drop for DropProbe {
        fn drop(&mut self) {
            *self.evidence.owner_drop.lock().expect("drop evidence") = Some(OwnerDropRecord {
                thread_id: thread::current().id(),
                child_finished: self.evidence.child_finished.load(Ordering::Acquire),
            });
        }
    }

    fn wait_until_shutdown(shutdown: &AtomicBool) -> bool {
        let deadline = Instant::now() + CHILD_DEADLINE;
        while !shutdown.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(1));
        }
        true
    }

    fn stopping_child(shutdown: Arc<AtomicBool>, finished: Arc<AtomicBool>) -> JoinHandle<()> {
        thread::spawn(move || {
            assert!(
                wait_until_shutdown(&shutdown),
                "child observed shutdown before its deadline"
            );
            finished.store(true, Ordering::Release);
        })
    }

    fn panicking_owner() -> DspThreadOwner<()> {
        let shutdown = Arc::new(AtomicBool::new(false));
        let handle: JoinHandle<()> = thread::spawn(|| panic!("simulated DSP child panic"));
        DspThreadOwner::new(handle, shutdown, Arc::new(()))
    }

    fn spawn_poisoned_broker(
        owner: Arc<Mutex<DropProbe>>,
        child_finished: Arc<AtomicBool>,
        child_saw_poison: Arc<AtomicBool>,
    ) -> PoisonedBrokerRun {
        let (observed_sender, child_observed_poison) = mpsc::sync_channel(1);
        let (release_child, release_receiver) = mpsc::sync_channel(1);
        let (creator_id_sender, creator_id) = mpsc::sync_channel(1);
        let (finished_sender, creator_finished) = mpsc::sync_channel(1);
        let creator = thread::spawn(move || {
            let creator_id = thread::current().id();
            creator_id_sender
                .send(creator_id)
                .expect("test remains active");
            let shutdown = Arc::new(AtomicBool::new(false));
            let child_shutdown = Arc::clone(&shutdown);
            let child_owner = Arc::clone(&owner);
            let child_done = Arc::clone(&child_finished);
            let child_poisoned = Arc::clone(&child_saw_poison);
            let handle = thread::spawn(move || {
                assert!(
                    wait_until_shutdown(&child_shutdown),
                    "child observed shutdown before its deadline"
                );
                let poisoned = child_owner.lock().is_err();
                child_poisoned.store(poisoned, Ordering::Release);
                observed_sender
                    .send(poisoned)
                    .expect("parent remains active until child is released");
                release_receiver
                    .recv_timeout(TEST_DEADLINE)
                    .expect("parent releases the child before its deadline");
                child_done.store(true, Ordering::Release);
            });

            let panic = catch_unwind(AssertUnwindSafe(|| {
                let _guard = DspThreadOwner::new(handle, shutdown, Arc::clone(&owner));
                let _broker_lock = owner.lock().expect("unpoisoned before broker panic");
                panic!("simulated broker panic");
            }));
            drop(owner);
            finished_sender
                .send(panic.is_err())
                .expect("parent observes creator completion");
        });

        PoisonedBrokerRun {
            creator,
            creator_id,
            child_observed_poison,
            creator_finished,
            release_child,
        }
    }

    #[test]
    fn repeated_stop_joins_once_and_drops_owner_on_creator_after_child_exit() {
        let creator = thread::current().id();
        let shutdown = Arc::new(AtomicBool::new(false));
        let child_finished = Arc::new(AtomicBool::new(false));
        let evidence = Arc::new(DropEvidence {
            child_finished: Arc::clone(&child_finished),
            owner_drop: Mutex::new(None),
        });
        let owner = Arc::new(Mutex::new(DropProbe {
            evidence: Arc::clone(&evidence),
        }));
        let handle = stopping_child(Arc::clone(&shutdown), Arc::clone(&child_finished));
        let mut guard = DspThreadOwner::new(handle, shutdown, owner);

        assert_eq!(guard.stop_and_join(), Ok(()));
        assert_eq!(guard.stop_and_join(), Ok(()));
        assert!(child_finished.load(Ordering::Acquire));
        assert_eq!(
            *evidence.owner_drop.lock().expect("drop evidence"),
            Some(OwnerDropRecord {
                thread_id: creator,
                child_finished: true,
            })
        );
    }

    #[test]
    fn broker_error_still_explicitly_stops_and_joins_child() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let child_finished = Arc::new(AtomicBool::new(false));
        let handle = stopping_child(Arc::clone(&shutdown), Arc::clone(&child_finished));
        let guard = DspThreadOwner::new(handle, shutdown, Arc::new(()));

        let result = guard.finish(Err("broker failed".to_owned()));

        assert_eq!(result, Err("broker failed".to_owned()));
        assert!(child_finished.load(Ordering::Acquire));
    }

    #[test]
    fn finish_preserves_broker_and_child_panic_errors_in_order() {
        let guard = panicking_owner();

        assert_eq!(
            guard.finish(Err("broker failed".to_owned())),
            Err("broker failed; processor DSP thread panicked".to_owned())
        );
    }

    #[test]
    fn broker_unwind_poisons_owner_but_drop_still_joins_child() {
        let child_finished = Arc::new(AtomicBool::new(false));
        let child_saw_poison = Arc::new(AtomicBool::new(false));
        let evidence = Arc::new(DropEvidence {
            child_finished: Arc::clone(&child_finished),
            owner_drop: Mutex::new(None),
        });
        let owner = Arc::new(Mutex::new(DropProbe {
            evidence: Arc::clone(&evidence),
        }));
        let run = spawn_poisoned_broker(
            owner,
            Arc::clone(&child_finished),
            Arc::clone(&child_saw_poison),
        );

        let creator_id = run
            .creator_id
            .recv_timeout(TEST_DEADLINE)
            .expect("creator starts before its deadline");
        assert!(
            run.child_observed_poison
                .recv_timeout(TEST_DEADLINE)
                .expect("child observes the poisoned owner")
        );
        assert!(matches!(
            run.creator_finished.recv_timeout(Duration::from_millis(75)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        run.release_child
            .send(())
            .expect("child still waits for release");
        assert!(
            run.creator_finished
                .recv_timeout(TEST_DEADLINE)
                .expect("creator finishes after child exit")
        );
        run.creator
            .join()
            .expect("creator thread completes normally");

        assert!(child_finished.load(Ordering::Acquire));
        assert!(child_saw_poison.load(Ordering::Acquire));
        assert_eq!(
            *evidence.owner_drop.lock().expect("drop evidence"),
            Some(OwnerDropRecord {
                thread_id: creator_id,
                child_finished: true,
            })
        );
    }

    #[test]
    fn child_panic_is_reported_and_guard_drop_never_repanics() {
        let mut guard = panicking_owner();
        assert_eq!(guard.stop_and_join(), Err(DspThreadError::ChildPanicked));
        drop(guard);

        let guard = panicking_owner();
        let drop_result = catch_unwind(AssertUnwindSafe(|| drop(guard)));
        assert!(drop_result.is_ok());
    }
}
