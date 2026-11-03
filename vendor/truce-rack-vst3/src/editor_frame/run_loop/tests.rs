//! Linux callback reentrancy and bounded registration ownership.

use super::*;
use crate::editor_frame::EditorFrame;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::time::Instant;
use vst3::Steinberg::Linux::{
    IEventHandler, IEventHandlerTrait, IRunLoop, IRunLoopTrait, ITimerHandler, ITimerHandlerTrait,
};
use vst3::{Class, ComWrapper};

struct ReentrantEventHandler {
    loop_ptr: AtomicPtr<IRunLoop>,
    self_ptr: AtomicPtr<IEventHandler>,
    calls: Arc<AtomicUsize>,
}

impl Class for ReentrantEventHandler {
    type Interfaces = (IEventHandler,);
}

impl IEventHandlerTrait for ReentrantEventHandler {
    unsafe fn onFDIsSet(&self, _fd: i32) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the loop pointer comes from the test's live frame wrapper and remains
        // valid for this synchronous callback. `ComRef` does not change its refcount.
        let Some(run_loop) =
            (unsafe { vst3::ComRef::<IRunLoop>::from_raw(self.loop_ptr.load(Ordering::Acquire)) })
        else {
            return;
        };
        let handler = self.self_ptr.load(Ordering::Acquire);
        // The unregister call reenters the registry while service has no lock held.
        unsafe { run_loop.unregisterEventHandler(handler) };
    }
}

#[test]
fn fd_callback_can_unregister_itself_without_holding_registry_lock() {
    let frame_wrapper = ComWrapper::new(EditorFrame::new());
    let run_loop = frame_wrapper
        .as_com_ref::<IRunLoop>()
        .expect("Linux run loop interface");
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_wrapper = ComWrapper::new(ReentrantEventHandler {
        loop_ptr: AtomicPtr::new(run_loop.as_ptr()),
        self_ptr: AtomicPtr::new(ptr::null_mut()),
        calls: Arc::clone(&calls),
    });
    let handler = handler_wrapper
        .as_com_ref::<IEventHandler>()
        .expect("event handler interface");
    handler_wrapper
        .self_ptr
        .store(handler.as_ptr(), Ordering::Release);

    let mut pipe_fds = [-1; 2];
    // SAFETY: `pipe_fds` has room for the two descriptors returned by pipe(2).
    assert_eq!(unsafe { libc::pipe(pipe_fds.as_mut_ptr()) }, 0);
    let byte = [1_u8];
    // SAFETY: the write descriptor is open and the input points to one initialized byte.
    assert_eq!(
        unsafe { libc::write(pipe_fds[1], byte.as_ptr().cast(), 1) },
        1
    );
    assert_eq!(
        unsafe { run_loop.registerEventHandler(handler.as_ptr(), pipe_fds[0]) },
        kResultOk
    );

    frame_wrapper.service_run_loop();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(frame_wrapper.run_loop.registrations().events.is_empty());

    assert_eq!(unsafe { libc::close(pipe_fds[0]) }, 0);
    assert_eq!(unsafe { libc::close(pipe_fds[1]) }, 0);
}

#[test]
fn run_loop_registration_is_bounded_and_close_clears_callbacks() {
    let state = RunLoopState::new();
    let drops = Arc::new(AtomicUsize::new(0));
    for index in 0..MAX_EVENT_HANDLERS {
        let handler = ComWrapper::new(DropCountHandler(Arc::clone(&drops)));
        let event_handler = handler
            .as_com_ref::<IEventHandler>()
            .expect("event handler interface");
        let fd = i32::try_from(index).expect("handler slot fits file descriptor");
        assert_eq!(
            state.register_event_handler(event_handler.as_ptr(), fd),
            kResultOk
        );
        if index < MAX_TIMER_HANDLERS {
            let timer_handler = handler
                .as_com_ref::<ITimerHandler>()
                .expect("timer handler interface");
            assert_eq!(
                state.register_timer(timer_handler.as_ptr(), 60_000),
                kResultOk
            );
        }
    }
    assert_eq!(state.registrations().events.len(), MAX_EVENT_HANDLERS);
    assert_eq!(state.registrations().timers.len(), MAX_TIMER_HANDLERS);
    assert_eq!(drops.load(Ordering::Relaxed), 0);

    let overflow = ComWrapper::new(DropCountHandler(Arc::clone(&drops)));
    let overflow_ref = overflow
        .as_com_ref::<IEventHandler>()
        .expect("event handler interface");
    assert_eq!(
        state.register_event_handler(overflow_ref.as_ptr(), 100),
        vst3::Steinberg::kResultFalse
    );
    drop(overflow);
    assert_eq!(drops.load(Ordering::Relaxed), 1);

    state.close();
    assert!(state.registrations().events.is_empty());
    assert!(state.registrations().timers.is_empty());
    assert_eq!(drops.load(Ordering::Relaxed), MAX_EVENT_HANDLERS + 1);
}

#[test]
fn timer_registration_rejects_after_teardown() {
    let frame_wrapper = ComWrapper::new(EditorFrame::new());
    let run_loop = frame_wrapper
        .as_com_ref::<IRunLoop>()
        .expect("Linux run loop interface");
    let timer = ComWrapper::new(DropCountHandler(Arc::new(AtomicUsize::new(0))));
    let timer_ref = timer
        .as_com_ref::<ITimerHandler>()
        .expect("timer handler interface");
    frame_wrapper.close_run_loop();
    assert_eq!(
        unsafe { run_loop.registerTimer(timer_ref.as_ptr(), 1) },
        vst3::Steinberg::kResultFalse
    );
}

struct ReentrantTimerHandler {
    loop_ptr: AtomicPtr<IRunLoop>,
    self_ptr: AtomicPtr<ITimerHandler>,
    calls: Arc<AtomicUsize>,
}

impl Class for ReentrantTimerHandler {
    type Interfaces = (ITimerHandler,);
}

impl ITimerHandlerTrait for ReentrantTimerHandler {
    unsafe fn onTimer(&self) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the loop pointer comes from the test's live frame wrapper and remains
        // valid for this synchronous callback. `ComRef` does not change its refcount.
        let Some(run_loop) =
            (unsafe { vst3::ComRef::<IRunLoop>::from_raw(self.loop_ptr.load(Ordering::Acquire)) })
        else {
            return;
        };
        let handler = self.self_ptr.load(Ordering::Acquire);
        // Timer callbacks may unregister themselves without reentering a held registry lock.
        unsafe { run_loop.unregisterTimer(handler) };
    }
}

#[test]
fn due_timer_callback_can_unregister_itself() {
    let frame_wrapper = ComWrapper::new(EditorFrame::new());
    let run_loop = frame_wrapper
        .as_com_ref::<IRunLoop>()
        .expect("Linux run loop interface");
    let calls = Arc::new(AtomicUsize::new(0));
    let timer_wrapper = ComWrapper::new(ReentrantTimerHandler {
        loop_ptr: AtomicPtr::new(run_loop.as_ptr()),
        self_ptr: AtomicPtr::new(ptr::null_mut()),
        calls: Arc::clone(&calls),
    });
    let timer = timer_wrapper
        .as_com_ref::<ITimerHandler>()
        .expect("timer handler interface");
    timer_wrapper
        .self_ptr
        .store(timer.as_ptr(), Ordering::Release);
    assert_eq!(
        unsafe { run_loop.registerTimer(timer.as_ptr(), 60_000) },
        kResultOk
    );
    frame_wrapper.run_loop.registrations().timers[0].due = Instant::now();

    frame_wrapper.service_run_loop();

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(frame_wrapper.run_loop.registrations().timers.is_empty());
}

struct DropCountHandler(Arc<AtomicUsize>);

impl Drop for DropCountHandler {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl Class for DropCountHandler {
    type Interfaces = (IEventHandler, ITimerHandler);
}

impl IEventHandlerTrait for DropCountHandler {
    unsafe fn onFDIsSet(&self, _fd: i32) {}
}

impl ITimerHandlerTrait for DropCountHandler {
    unsafe fn onTimer(&self) {}
}
