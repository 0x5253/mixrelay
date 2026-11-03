use super::*;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize},
    mpsc,
};

#[derive(Default)]
struct Shared {
    blocked: Mutex<bool>,
    resume: Condvar,
    entered: AtomicBool,
    fail: AtomicBool,
    calls: AtomicUsize,
    failed_calls: AtomicUsize,
}

struct AsyncNative(Arc<Shared>);

impl Native for AsyncNative {
    fn device(&self, _: &std::ffi::CStr) -> Result<native::DeviceLease, TapError> {
        panic!("retirement never resolves a device")
    }
    fn processes(
        &self,
        _: &std::ffi::CStr,
        _: Option<u32>,
    ) -> Result<Vec<native::ProcessIdentity>, TapError> {
        panic!("retirement never discovers processes")
    }
    fn rebuild(&self, _: &TapSession, _: &native::Selection) -> Result<(), TapError> {
        panic!("retirement never rebuilds a tap")
    }
    fn format(&self, _: u32) -> Result<TapFormat, TapError> {
        Ok(format())
    }
    fn start(&self, _: u32, _: *mut c_void) -> Result<(), TapError> {
        Ok(())
    }
    fn stop(&self, _: u32, _: *mut c_void) -> Result<(), TapError> {
        Ok(())
    }
    fn register(&self, session: &TapSession) -> Result<(), TapError> {
        session.adopt_registration(ptr::dangling_mut(), 0)
    }
    fn unregister(&self, _: u32, _: *mut c_void) -> Result<(), TapError> {
        self.0.calls.fetch_add(1, Ordering::Relaxed);
        self.0.entered.store(true, Ordering::Release);
        let mut blocked = self.0.blocked.lock().expect("test gate");
        while *blocked {
            blocked = self.0.resume.wait(blocked).expect("test gate");
        }
        if self.0.fail.load(Ordering::Acquire) {
            // Publish after selecting failure; the observer cannot change this call's outcome.
            self.0.failed_calls.fetch_add(1, Ordering::Release);
            TapError::status(-1)
        } else {
            Ok(())
        }
    }
    fn remove_listener(&self, _: u32, _: &TapSession) -> Result<(), TapError> {
        Ok(())
    }
    fn remove_inventory_listener(&self, _: &TapSession) -> Result<(), TapError> {
        panic!("retirement fixture has no inventory listener")
    }
    fn remove_service_listener(&self, _: &TapSession) -> Result<(), TapError> {
        panic!("retirement fixture has no service listener")
    }
    fn install_system_listeners(&self, _: &TapSession) -> Result<(), TapError> {
        panic!("retirement never installs listeners")
    }
    fn destroy_aggregate(&self, _: u32) -> Result<(), TapError> {
        Ok(())
    }
    fn destroy_tap(&self, _: u32) -> Result<(), TapError> {
        Ok(())
    }
    fn diagnostic(
        &self,
        _: &native::Selection,
        _: std::time::Instant,
    ) -> Result<crate::watchdog::Diagnostic, TapError> {
        panic!("retirement never creates a diagnostic")
    }
    fn diagnostic_has_audio(&self, _: &crate::watchdog::Diagnostic) -> bool {
        panic!("retirement never observes a diagnostic")
    }
    fn retire_diagnostic(&self, _: crate::watchdog::Diagnostic) {
        panic!("retirement fixture has no diagnostic")
    }
}

struct Owner {
    session: TapSession,
    native: AsyncNative,
    done: mpsc::Sender<()>,
}

unsafe extern "C" fn cleanup(context: *mut c_void) {
    // SAFETY: the test transfers this heap owner exclusively to the production
    // serial dispatcher. Only this callback and its own delayed retry access it.
    let owner = unsafe { &*context.cast::<Owner>() };
    if owner.session.reclaim(&owner.native) {
        let done = owner.done.clone();
        // SAFETY: exact production resource reclamation succeeded; neither native
        // fixture operation nor test observer retains session memory.
        drop(unsafe { Box::from_raw(context.cast::<Owner>()) });
        done.send(()).expect("test observer retained");
    } else {
        // SAFETY: failed removal keeps this same queue's allocation/admission alive.
        unsafe { native::enqueue(owner.session.retirement_queue, context, cleanup, true) };
    }
}

fn wait_for(condition: impl Fn() -> bool) {
    let begin = std::time::Instant::now();
    while !condition() {
        assert!(begin.elapsed() < std::time::Duration::from_secs(3));
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn real_dispatch_queue_does_not_block_destroy_and_retries_before_admission_release() {
    let shared = Arc::new(Shared::default());
    *shared.blocked.lock().expect("test gate") = true;
    shared.fail.store(true, Ordering::Release);
    let mut session = fixture();
    session.retirement_queue = native::prepare_cleanup().expect("serial queue");
    assert!(session.lifecycle.acquire_owner());
    assert!(session.lifecycle.retire());
    assert!(!session.lifecycle.retire());
    assert!(!session.allowed());
    let queue = session.retirement_queue;
    let (done, finished) = mpsc::channel();
    let owner = Box::new(Owner {
        session,
        native: AsyncNative(Arc::clone(&shared)),
        done,
    });
    // SAFETY: production dispatcher receives one uniquely owned heap allocation;
    // the test retains only atomic mock state, never the session after transfer.
    unsafe { native::enqueue(queue, Box::into_raw(owner).cast(), cleanup, false) };
    wait_for(|| shared.entered.load(Ordering::Acquire));
    assert!(finished.try_recv().is_err());
    let probes: [TapLifecycle; 32] = std::array::from_fn(|_| TapLifecycle::default());
    let held = probes.iter().filter(|gate| gate.acquire_owner()).count();
    assert_eq!(held, 31);
    for gate in &probes {
        gate.release_owner();
    }
    *shared.blocked.lock().expect("test gate") = false;
    shared.resume.notify_one();
    wait_for(|| shared.failed_calls.load(Ordering::Acquire) >= 2);
    assert!(finished.try_recv().is_err());
    shared.fail.store(false, Ordering::Release);
    finished
        .recv_timeout(std::time::Duration::from_secs(3))
        .expect("retry reclaimed owner");
    assert!(shared.calls.load(Ordering::Relaxed) >= 3);
    assert!(probes.iter().all(TapLifecycle::acquire_owner));
    for gate in &probes {
        gate.release_owner();
    }
}
