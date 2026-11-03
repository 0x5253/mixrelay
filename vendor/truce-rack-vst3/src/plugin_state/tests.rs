use super::{MAGIC, MAX_PLUGIN_STATE_BYTES, PluginState, PluginStateError, VERSION, encode};
use crate::state_stream::{STATE_STREAM_CAPACITY, StreamError};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use vst3::ComRef;
use vst3::Steinberg::{IBStream, IBStreamTrait, kNotImplemented, kResultFalse, kResultOk};

fn write_stream(stream: *mut IBStream, bytes: &[u8]) -> i32 {
    let (status, written) = write_stream_result(stream, bytes);
    assert_eq!(written, i32::try_from(bytes.len()).expect("fixture fits"));
    status
}

fn write_stream_result(stream: *mut IBStream, bytes: &[u8]) -> (i32, i32) {
    // SAFETY: capture_stream passes a live IBStream pointer for this synchronous callback.
    let stream = unsafe { ComRef::<IBStream>::from_raw(stream) }.expect("valid state stream");
    let length = i32::try_from(bytes.len()).expect("fixture fits the VST3 stream length");
    let mut written = 0;
    // SAFETY: `bytes` remains alive for the call and has `length` readable bytes.
    let status = unsafe {
        stream.write(
            bytes.as_ptr().cast_mut().cast::<c_void>(),
            length,
            &raw mut written,
        )
    };
    (status, written)
}

fn read_stream(stream: *mut IBStream, length: usize) -> (i32, Vec<u8>) {
    // SAFETY: restore_stream passes a live IBStream pointer for this synchronous callback.
    let stream = unsafe { ComRef::<IBStream>::from_raw(stream) }.expect("valid state stream");
    let byte_count = i32::try_from(length).expect("fixture fits the VST3 stream length");
    let mut output = vec![0; length];
    let mut read = 0;
    // SAFETY: the output allocation is writable for `byte_count` bytes during the call.
    let status = unsafe {
        stream.read(
            output.as_mut_ptr().cast::<c_void>(),
            byte_count,
            &raw mut read,
        )
    };
    let read = usize::try_from(read).expect("VST3 read count is nonnegative");
    output.truncate(read);
    (status, output)
}

fn ignore_seek_failure(stream: *mut IBStream) -> i32 {
    // SAFETY: restore/capture supplies a live stream for this synchronous fake plug-in call.
    let stream = unsafe { ComRef::<IBStream>::from_raw(stream) }.expect("valid state stream");
    let position = i64::try_from(STATE_STREAM_CAPACITY).expect("state capacity fits in i64") + 1;
    let mut result = 0;
    // SAFETY: result is writable; this intentionally exercises a rejected seek beyond capacity.
    let _ = unsafe { stream.seek(position, 0, &raw mut result) };
    // Simulate a plug-in that ignores the stream error and claims success.
    kResultOk
}

#[test]
fn capture_keeps_component_and_controller_payloads_distinct_and_ordered() {
    let events = RefCell::new(Vec::new());
    let bytes = PluginState::capture(
        |stream| {
            events.borrow_mut().push("component.getState");
            write_stream(stream, b"processor")
        },
        |stream| {
            events.borrow_mut().push("controller.getState");
            write_stream(stream, b"editor")
        },
    )
    .expect("both native state sections fit");

    assert_eq!(
        *events.borrow(),
        ["component.getState", "controller.getState"]
    );
    let decoded = PluginState::decode(&bytes).expect("captured envelope is valid");
    assert_eq!(decoded.component, b"processor");
    assert_eq!(decoded.controller, Some(&b"editor"[..]));
}

#[test]
fn unsupported_controller_state_is_absent_but_native_failure_is_not() {
    let absent = PluginState::capture(
        |stream| write_stream(stream, b"component"),
        |_stream| kNotImplemented,
    )
    .expect("unsupported optional controller state is represented as absent");
    assert_eq!(
        PluginState::decode(&absent)
            .expect("valid absent-controller envelope")
            .controller,
        None
    );

    let failure = PluginState::capture(
        |stream| write_stream(stream, b"component"),
        |_stream| kResultFalse,
    );
    assert!(matches!(
        failure,
        Err(PluginStateError::NativeStatus {
            operation: "IEditController::getState",
            status
        }) if status == kResultFalse
    ));
}

#[test]
fn unsupported_controller_state_with_partial_bytes_is_an_error() {
    let result = PluginState::capture(
        |stream| write_stream(stream, b"component"),
        |stream| {
            let status = write_stream(stream, b"partial");
            assert_eq!(status, kResultOk);
            kNotImplemented
        },
    );

    assert_eq!(
        result,
        Err(PluginStateError::UnsupportedControllerStateWithData)
    );
}

#[test]
fn restore_uses_sdk_order_and_allows_documented_component_sync_false() {
    let bytes = encode(b"processor", Some(b"editor")).expect("fixture envelope fits");
    let events = RefCell::new(Vec::new());
    let result = PluginState::restore(
        &bytes,
        || events.borrow_mut().push("invalidate"),
        |stream| {
            events.borrow_mut().push("component.setState");
            let (status, payload) = read_stream(stream, b"processor".len());
            assert_eq!(status, kResultOk);
            assert_eq!(payload, b"processor");
            kResultOk
        },
        |stream| {
            events.borrow_mut().push("controller.setComponentState");
            let (status, payload) = read_stream(stream, b"processor".len());
            assert_eq!(status, kResultOk);
            assert_eq!(payload, b"processor");
            kResultFalse
        },
        |stream| {
            events.borrow_mut().push("controller.setState");
            let (status, payload) = read_stream(stream, b"editor".len());
            assert_eq!(status, kResultOk);
            assert_eq!(payload, b"editor");
            kResultOk
        },
    );

    assert_eq!(result, Ok(()));
    assert_eq!(
        *events.borrow(),
        [
            "invalidate",
            "component.setState",
            "controller.setComponentState",
            "controller.setState"
        ]
    );
}

#[test]
fn unsupported_component_sync_continues_to_private_controller_state() {
    let bytes = encode(b"component", Some(b"controller")).expect("fixture envelope fits");
    let restored_controller = Cell::new(false);
    let result = PluginState::restore(
        &bytes,
        || {},
        |_stream| kResultOk,
        |_stream| kNotImplemented,
        |stream| {
            let (status, payload) = read_stream(stream, b"controller".len());
            assert_eq!(status, kResultOk);
            assert_eq!(payload, b"controller");
            restored_controller.set(true);
            kResultOk
        },
    );
    assert_eq!(result, Ok(()));
    assert!(restored_controller.get());
}

#[test]
fn unsupported_state_restore_is_fatal_outside_component_sync() {
    let bytes = encode(b"component", Some(b"controller")).expect("fixture envelope fits");
    for (component_status, controller_status, operation) in [
        (kNotImplemented, kResultOk, "IComponent::setState"),
        (kResultOk, kNotImplemented, "IEditController::setState"),
    ] {
        assert_eq!(
            PluginState::restore(
                &bytes,
                || {},
                |_stream| component_status,
                |_stream| kResultOk,
                |_stream| controller_status,
            ),
            Err(PluginStateError::NativeStatus {
                operation,
                status: kNotImplemented,
            })
        );
    }
}

#[test]
fn unsupported_component_sync_does_not_hide_stream_failure() {
    let bytes = encode(b"component", None).expect("fixture envelope fits");
    let result = PluginState::restore(
        &bytes,
        || {},
        |_stream| kResultOk,
        |stream| {
            ignore_seek_failure(stream);
            kNotImplemented
        },
        |_stream| kResultOk,
    );
    assert_eq!(
        result,
        Err(PluginStateError::Stream(StreamError::CapacityExceeded))
    );
}

#[test]
fn malformed_or_oversized_envelopes_fail_before_any_native_mutation() {
    let valid = encode(b"component", None).expect("fixture envelope fits");
    assert_eq!(
        PluginState::decode(b"raw component state fixture"),
        Err(PluginStateError::BadMagic)
    );
    assert_eq!(
        PluginState::decode(&valid[..8]),
        Err(PluginStateError::Truncated)
    );
    let truncated = &valid[..valid.len() - 1];
    let invalidations = Cell::new(0);
    let native_calls = Cell::new(0);
    let result = PluginState::restore(
        truncated,
        || invalidations.set(invalidations.get() + 1),
        |_stream| {
            native_calls.set(native_calls.get() + 1);
            kResultOk
        },
        |_stream| {
            native_calls.set(native_calls.get() + 1);
            kResultOk
        },
        |_stream| {
            native_calls.set(native_calls.get() + 1);
            kResultOk
        },
    );
    assert_eq!(result, Err(PluginStateError::LengthMismatch));

    let mut oversized = vec![0; super::HEADER_LEN];
    oversized[..MAGIC.len()].copy_from_slice(MAGIC);
    oversized[8..10].copy_from_slice(&VERSION.to_le_bytes());
    oversized[10] = 0;
    oversized[12..16].copy_from_slice(
        &u32::try_from(MAX_PLUGIN_STATE_BYTES)
            .expect("the state transport capacity fits in u32")
            .to_le_bytes(),
    );
    let result = PluginState::restore(
        &oversized,
        || invalidations.set(invalidations.get() + 1),
        |_stream| {
            native_calls.set(native_calls.get() + 1);
            kResultOk
        },
        |_stream| {
            native_calls.set(native_calls.get() + 1);
            kResultOk
        },
        |_stream| {
            native_calls.set(native_calls.get() + 1);
            kResultOk
        },
    );
    assert_eq!(result, Err(PluginStateError::CapacityExceeded));
    assert_eq!(invalidations.get(), 0);
    assert_eq!(native_calls.get(), 0);
}

#[test]
fn native_restore_failure_reports_partial_restore_and_stops_following_calls() {
    let bytes = encode(b"component", Some(b"controller")).expect("fixture envelope fits");
    let events = RefCell::new(Vec::new());
    let invalidated = Cell::new(false);
    let result = PluginState::restore(
        &bytes,
        || invalidated.set(true),
        |_stream| {
            events.borrow_mut().push("component");
            kResultFalse
        },
        |_stream| {
            events.borrow_mut().push("set-component-state");
            kResultOk
        },
        |_stream| {
            events.borrow_mut().push("controller-state");
            kResultOk
        },
    );

    assert_eq!(
        result,
        Err(PluginStateError::NativeStatus {
            operation: "IComponent::setState",
            status: kResultFalse
        })
    );
    assert!(invalidated.get());
    assert_eq!(*events.borrow(), ["component"]);
}

#[test]
fn controller_state_failure_is_reported_after_component_synchronization() {
    let bytes = encode(b"component", Some(b"controller")).expect("fixture envelope fits");
    let events = RefCell::new(Vec::new());
    let result = PluginState::restore(
        &bytes,
        || events.borrow_mut().push("invalidate"),
        |_stream| {
            events.borrow_mut().push("component");
            kResultOk
        },
        |_stream| {
            events.borrow_mut().push("set-component-state");
            vst3::Steinberg::kInvalidArgument
        },
        |_stream| {
            events.borrow_mut().push("controller-state");
            kResultFalse
        },
    );

    assert_eq!(
        result,
        Err(PluginStateError::NativeStatus {
            operation: "IEditController::setComponentState",
            status: vst3::Steinberg::kInvalidArgument
        })
    );
    assert_eq!(
        *events.borrow(),
        ["invalidate", "component", "set-component-state"]
    );
}

#[test]
fn sticky_stream_errors_override_success_status_during_capture_and_restore() {
    let capture = PluginState::capture(ignore_seek_failure, |_stream| kNotImplemented);
    assert_eq!(
        capture,
        Err(PluginStateError::Stream(StreamError::CapacityExceeded))
    );

    let bytes = encode(b"component", None).expect("fixture envelope fits");
    let restore = PluginState::restore(
        &bytes,
        || {},
        ignore_seek_failure,
        |_stream| kResultOk,
        |_stream| kResultOk,
    );
    assert_eq!(
        restore,
        Err(PluginStateError::Stream(StreamError::CapacityExceeded))
    );
}

#[test]
fn controller_capture_is_limited_to_the_remaining_aggregate_budget() {
    let aggregate_capacity = super::HEADER_LEN + b"component".len() + 2;
    let result = PluginState::capture_with_budget(
        |stream| write_stream(stream, b"component"),
        |stream| {
            let (status, written) = write_stream_result(stream, b"too large");
            assert_eq!(status, vst3::Steinberg::kOutOfMemory);
            assert_eq!(written, 0);
            kResultOk
        },
        aggregate_capacity,
    );

    assert_eq!(
        result,
        Err(PluginStateError::Stream(StreamError::CapacityExceeded))
    );
}

#[test]
fn state_length_arithmetic_rejects_overflow_without_allocating() {
    assert_eq!(
        super::checked_total_len(usize::MAX, 1),
        Err(PluginStateError::CapacityExceeded)
    );
}
