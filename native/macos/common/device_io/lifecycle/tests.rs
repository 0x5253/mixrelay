use std::{ffi::c_void, ptr::NonNull};

use super::{BAD_DEVICE, BAD_OBJECT, Backend, Created, DeviceIoLifecycle, ILLEGAL_OPERATION};

#[derive(Default)]
struct Native {
    calls: Vec<&'static str>,
    prepared_device: u32,
    expected: u32,
    prepare_error: Option<i32>,
    create_status: i32,
    omit_handle: bool,
    start_status: i32,
    destroy_status: i32,
    stop_status: i32,
    stopped: Option<(u32, NonNull<c_void>)>,
    destroyed: Option<(u32, NonNull<c_void>)>,
}

impl Backend for Native {
    fn prepare(&mut self, expected: u32) -> Result<u32, i32> {
        self.calls.push("prepare");
        self.expected = expected;
        self.prepare_error.map_or(Ok(self.prepared_device), Err)
    }
    fn create(&mut self, _device: u32) -> Created {
        self.calls.push("create");
        Created {
            procedure: (!self.omit_handle).then(NonNull::dangling),
            status: self.create_status,
        }
    }
    fn start(&mut self, _device: u32, _procedure: NonNull<c_void>) -> i32 {
        self.calls.push("start");
        self.start_status
    }
    fn silence(&mut self) {
        self.calls.push("silence");
    }
    fn stop(&mut self, device: u32, procedure: NonNull<c_void>) -> i32 {
        self.calls.push("stop");
        self.stopped = Some((device, procedure));
        self.stop_status
    }
    fn destroy(&mut self, device: u32, procedure: NonNull<c_void>) -> i32 {
        self.calls.push("destroy");
        self.destroyed = Some((device, procedure));
        self.destroy_status
    }
}

fn backend() -> Native {
    Native {
        prepared_device: 37,
        ..Native::default()
    }
}

#[test]
fn empty_owner_is_disconnected_and_retirement_calls_nothing() {
    let mut state = DeviceIoLifecycle::default();
    let mut native = backend();
    assert!(state.disconnect(&mut native));
    assert!(state.procedure.is_none());
    assert!(native.calls.is_empty());
}

#[test]
fn connect_publishes_handle_and_rejects_duplicate_without_preparing() {
    let mut state = DeviceIoLifecycle::default();
    let mut native = backend();
    assert_eq!(state.connect(37, &mut native), 0);
    assert_eq!(native.expected, 37);
    assert_eq!(state.device, 37);
    assert_eq!(state.procedure, Some(NonNull::dangling()));
    assert_eq!(state.connect(42, &mut native), -1);
    assert_eq!(native.calls, ["prepare", "create", "start"]);
}

#[test]
fn preparation_failure_does_not_create_or_start_a_callback() {
    for error in [-2, -3, -5] {
        let mut state = DeviceIoLifecycle::default();
        let mut native = Native {
            prepare_error: Some(error),
            ..backend()
        };
        assert_eq!(state.connect(0, &mut native), error);
        assert!(state.procedure.is_none());
        assert_eq!(state.device, 0);
        assert_eq!(native.calls, ["prepare"]);
    }
}

#[test]
fn create_error_with_handle_retains_registration_until_confirmed_destroy() {
    let mut state = DeviceIoLifecycle::default();
    let mut native = Native {
        create_status: 17,
        destroy_status: 23,
        ..backend()
    };
    assert_eq!(state.connect(0, &mut native), 17);
    assert_eq!(native.calls, ["prepare", "create"]);
    assert!(!state.disconnect(&mut native));
    assert_eq!(state.device, 37);
    assert_eq!(state.procedure, Some(NonNull::dangling()));
    assert_eq!(native.destroyed, Some((37, NonNull::dangling())));
    native.destroy_status = 0;
    assert!(state.disconnect(&mut native));
    assert!(state.procedure.is_none());
}

#[test]
fn create_error_without_handle_allows_retry_and_null_success_is_not_connected() {
    let mut state = DeviceIoLifecycle::default();
    let mut native = Native {
        create_status: 17,
        omit_handle: true,
        ..backend()
    };
    assert_eq!(state.connect(0, &mut native), 17);
    assert!(state.disconnect(&mut native));
    native.create_status = 0;
    assert_eq!(state.connect(0, &mut native), ILLEGAL_OPERATION);
    assert!(!native.calls.contains(&"start"));
    native.omit_handle = false;
    assert_eq!(state.connect(0, &mut native), 0);
}

#[test]
fn start_failure_silences_then_attempts_retirement_without_losing_original_error() {
    for destroy_status in [0, 23] {
        let mut state = DeviceIoLifecycle::default();
        let mut native = Native {
            start_status: 19,
            destroy_status,
            ..backend()
        };
        assert_eq!(state.connect(0, &mut native), 19);
        assert_eq!(
            native.calls,
            ["prepare", "create", "start", "silence", "stop", "destroy"]
        );
        assert_eq!(state.procedure.is_some(), destroy_status != 0);
        assert_eq!(native.stopped, Some((37, NonNull::dangling())));
    }
}

#[test]
fn failed_stop_does_not_prevent_successful_destroy_or_deleted_object_acknowledgement() {
    for destroy_status in [0, BAD_DEVICE, BAD_OBJECT] {
        let mut state = DeviceIoLifecycle::default();
        let mut native = Native {
            destroy_status,
            stop_status: 99,
            ..backend()
        };
        assert_eq!(state.connect(0, &mut native), 0);
        assert!(state.disconnect(&mut native));
        assert_eq!(state.device, 0);
        assert!(state.procedure.is_none());
        assert_eq!(
            native.calls,
            ["prepare", "create", "start", "silence", "stop", "destroy"]
        );
    }
}

#[test]
fn failed_destroy_keeps_exact_original_pair_across_retry_and_prevents_reconnection() {
    let mut state = DeviceIoLifecycle::default();
    let mut native = Native {
        destroy_status: 23,
        ..backend()
    };
    assert_eq!(state.connect(0, &mut native), 0);
    assert!(!state.disconnect(&mut native));
    native.prepared_device = 41;
    assert_eq!(state.connect(0, &mut native), -1);
    assert_eq!(state.device, 37);
    native.destroy_status = 0;
    assert!(state.disconnect(&mut native));
    assert_eq!(native.destroyed, Some((37, NonNull::dangling())));
    assert_eq!(state.connect(0, &mut native), 0);
    assert_eq!(state.device, 41);
}
