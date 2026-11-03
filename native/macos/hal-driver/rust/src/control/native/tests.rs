use super::*;
use std::{sync::mpsc, time::Duration};

unsafe extern "C" {
    fn dispatch_set_finalizer_f(object: Handle, callback: Callback);
}

#[test]
fn failed_queue_creation_never_attempts_timer_or_callback_publication() {
    assert!(Native::with_queue(None, |_| panic!("no queue exists")).is_none());
}

#[test]
fn failed_timer_creation_releases_unpublished_queue() {
    unsafe extern "C" fn finalized(context: Handle) {
        // SAFETY: this queue alone owns the boxed sender after set_context.
        let sender = unsafe { Box::from_raw(context.cast::<mpsc::Sender<()>>()) };
        let _ = sender.send(());
    }
    // This creates only an anonymous serial queue, never a connection or callback.
    // SAFETY: static label and null serial attributes obey the dispatch ABI.
    let queue =
        unsafe { dispatch_queue_create(c"io.mixrelay.test.control".as_ptr(), ptr::null_mut()) };
    let queue = Dispatch(NonNull::new(queue).unwrap());
    let (sender, receiver) = mpsc::channel::<()>();
    // SAFETY: the queue owns the sender until its one finalizer callback. No
    // event handler or work has been registered on this private test queue.
    unsafe {
        dispatch_set_context(queue.0.as_ptr(), Box::into_raw(Box::new(sender)).cast());
        dispatch_set_finalizer_f(queue.0.as_ptr(), finalized);
    }
    let mut attempted = false;
    let owner = Native::with_queue(Some(queue), |pointer| {
        assert!(!pointer.is_null());
        attempted = true;
        None
    });
    assert!(attempted);
    assert!(owner.is_none());
    receiver.recv_timeout(Duration::from_secs(1)).unwrap();
}

#[test]
fn unpublished_timer_can_be_reclaimed_without_resuming_work() {
    // No publish call means no handler, registry, XPC connection or audio device.
    for _ in 0..100 {
        drop(Native::new().expect("allocate local dispatch queue and timer"));
    }
}
