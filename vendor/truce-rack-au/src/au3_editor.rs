//! `AUv3`'s view-controller property and its asynchronous ownership boundary.

use std::{
    ptr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::NSViewController;
use objc2_audio_toolbox::{
    AudioComponent, AudioComponentCopyConfigurationInfo, AudioComponentInstance,
    AudioUnitSetProperty, kAudioUnitProperty_RequestViewController, kAudioUnitScope_Global,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFRetained, CFString, CFType};
use truce_rack_core::error::{Error, Result};

/// The component's own configuration dictionary reports whether it has a custom view.
/// A missing value or a value of the wrong Core Foundation type is not a true capability.
///
/// # Safety
/// `component` must be a valid, live Audio Component handle.
pub(super) unsafe fn has_custom_view(component: AudioComponent) -> bool {
    let mut raw: *const CFDictionary = ptr::null();
    let status =
        unsafe { AudioComponentCopyConfigurationInfo(component, ptr::NonNull::from(&mut raw)) };
    let Some(raw) = ptr::NonNull::new(raw.cast_mut()) else {
        return false;
    };
    // SAFETY: a non-null pointer returned by this Copy API is owned by the caller.
    let dictionary = unsafe { CFRetained::<CFDictionary>::from_raw(raw) };
    status == 0 && custom_view_flag(&dictionary)
}

fn custom_view_flag(dictionary: &CFDictionary) -> bool {
    let key = CFString::from_static_str("HasCustomView");
    let raw = unsafe { dictionary.value(CFRetained::as_ptr(&key).as_ptr().cast()) };
    let Some(raw) = ptr::NonNull::new(raw.cast_mut()) else {
        return false;
    };
    // SAFETY: the dictionary owns this value until `dictionary` is dropped. Check its
    // runtime type before interpreting it as the SDK's documented CFBoolean.
    let value = unsafe { raw.cast::<CFType>().as_ref() };
    value
        .downcast_ref::<CFBoolean>()
        .is_some_and(CFBoolean::value)
}

#[derive(Default)]
struct ViewRequest {
    state: Mutex<RequestState>,
}

#[derive(Default)]
enum RequestState {
    #[default]
    Pending,
    Completed(ViewCompletion),
    Cancelled,
}

#[derive(Debug, PartialEq, Eq)]
enum ViewCompletion {
    Controller(usize),
    NoCustomView,
}

impl ViewRequest {
    /// Consume the callback's Copy-rule reference, returning it only if the
    /// request was already cancelled or completed.
    fn complete(&self, raw: usize) -> Option<usize> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*state, RequestState::Pending) {
            *state = RequestState::Completed(if raw == 0 {
                ViewCompletion::NoCustomView
            } else {
                ViewCompletion::Controller(raw)
            });
            None
        } else {
            (raw != 0).then_some(raw)
        }
    }

    fn take(&self) -> Option<ViewCompletion> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match std::mem::replace(&mut *state, RequestState::Cancelled) {
            RequestState::Completed(owned) => Some(owned),
            RequestState::Pending => {
                *state = RequestState::Pending;
                None
            }
            RequestState::Cancelled => None,
        }
    }

    fn cancel(&self) -> Option<usize> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match std::mem::replace(&mut *state, RequestState::Cancelled) {
            RequestState::Completed(ViewCompletion::Controller(raw)) => Some(raw),
            RequestState::Completed(ViewCompletion::NoCustomView)
            | RequestState::Pending
            | RequestState::Cancelled => None,
        }
    }
}

/// Requests the `AUv3` controller while pumping the worker's main run loop. The callback
/// captures only `ViewRequest`, so a completion after timeout or unit disposal cannot access
/// the plug-in. Each non-null callback result is a Copy-rule reference, either transferred
/// to the main-thread waiter or released if the request has been cancelled.
///
/// # Safety
/// `unit` must remain live and exclusively controlled until this function returns.
/// The request must be initiated on the macOS main thread.
pub(super) unsafe fn request_view_controller(
    unit: AudioComponentInstance,
) -> Result<Retained<NSViewController>> {
    let _main = MainThreadMarker::new()
        .ok_or_else(|| Error::Other("AUv3 editor request requires macOS main thread".into()))?;
    let completion = Arc::new(ViewRequest::default());
    let callback_completion = Arc::clone(&completion);
    let block = RcBlock::new(move |controller: *mut NSViewController| {
        // The SDK explicitly gives this callback result to the host under the Copy
        // rule. A late or duplicate completion has no waiter and must release it.
        release_owned(callback_completion.complete(controller as usize));
    });
    let block_pointer = RcBlock::as_ptr(&block);
    let status = unsafe {
        AudioUnitSetProperty(
            unit,
            kAudioUnitProperty_RequestViewController,
            kAudioUnitScope_Global,
            0,
            (&raw const block_pointer).cast(),
            u32::try_from(std::mem::size_of_val(&block_pointer))
                .expect("block pointer size fits AudioUnit property size"),
        )
    };
    if status != 0 {
        release_owned(completion.cancel());
        return Err(Error::Other(format!(
            "AudioUnitSetProperty(RequestViewController) failed: OSStatus {status}"
        )));
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(result) = completion.take() {
            return match result {
                ViewCompletion::Controller(raw) => {
                    // SAFETY: the callback transferred its Copy-rule +1 reference to
                    // the request, and take transfers it to this creator/main thread.
                    unsafe { Retained::from_raw(raw as *mut NSViewController) }
                        .ok_or_else(|| Error::Other("AUv3 controller ownership lost".into()))
                }
                ViewCompletion::NoCustomView => {
                    Err(Error::Other("AUv3 has no custom view controller".into()))
                }
            };
        }
        if Instant::now() >= deadline {
            release_owned(completion.cancel());
            return Err(Error::Other(
                "AUv3 view-controller request timed out after 10s".into(),
            ));
        }
        let mode = unsafe { objc2_core_foundation::kCFRunLoopDefaultMode };
        objc2_core_foundation::CFRunLoop::run_in_mode(mode, 0.05, true);
    }
}

fn release_owned(raw: Option<usize>) {
    release_owned_with(
        raw,
        MainThreadMarker::new().is_some(),
        release_controller_on_main,
        |raw| {
            // The callback may run on an AU-internal queue, including after the plug-in
            // instance is disposed. Only its Copy-rule +1 controller reference crosses
            // this boundary; no plug-in, window, or instance pointer is captured.
            DispatchQueue::main().exec_async(move || release_controller_on_main(raw));
        },
    );
}

fn release_owned_with(
    raw: Option<usize>,
    on_main: bool,
    release_now: impl FnOnce(usize),
    dispatch_to_main: impl FnOnce(usize),
) {
    if let Some(raw) = raw {
        if on_main {
            release_now(raw);
        } else {
            dispatch_to_main(raw);
        }
    }
}

fn release_controller_on_main(raw: usize) {
    let _main = MainThreadMarker::new().expect("AUv3 controller release requires main thread");
    // SAFETY: `complete` gives exactly one Copy-rule +1 reference to either the
    // waiter or this cleanup path. This function runs only on the main thread,
    // including for late/duplicate completions from AU-internal callback queues.
    drop(unsafe { Retained::<NSViewController>::from_raw(raw as *mut NSViewController) });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_view_flag_checks_boolean_type_and_value() {
        let key = CFString::from_static_str("HasCustomView");
        let true_value = CFBoolean::new(true);
        let false_value = CFBoolean::new(false);
        let wrong_type = CFString::from_static_str("true");
        let yes =
            CFDictionary::<CFType, CFType>::from_slices(&[key.as_ref()], &[true_value.as_ref()]);
        let no =
            CFDictionary::<CFType, CFType>::from_slices(&[key.as_ref()], &[false_value.as_ref()]);
        let invalid =
            CFDictionary::<CFType, CFType>::from_slices(&[key.as_ref()], &[wrong_type.as_ref()]);
        assert!(custom_view_flag(yes.as_opaque()));
        assert!(!custom_view_flag(no.as_opaque()));
        assert!(!custom_view_flag(invalid.as_opaque()));
        assert!(!custom_view_flag(
            CFDictionary::<CFType, CFType>::empty().as_opaque()
        ));
    }

    #[test]
    fn completion_transfers_once_and_rejects_late_result() {
        let request = ViewRequest::default();
        assert_eq!(request.complete(41), None);
        assert_eq!(request.take(), Some(ViewCompletion::Controller(41)));
        assert_eq!(request.complete(42), Some(42));
        assert_eq!(request.take(), None);
        assert_eq!(request.cancel(), None);
    }

    #[test]
    fn cancellation_releases_completed_owner_and_rejects_late_completion() {
        let request = ViewRequest::default();
        assert_eq!(request.complete(41), None);
        assert_eq!(request.cancel(), Some(41));
        assert_eq!(request.complete(42), Some(42));
        assert_eq!(request.cancel(), None);

        let pending = ViewRequest::default();
        assert_eq!(pending.cancel(), None);
        assert_eq!(pending.complete(43), Some(43));
    }

    #[test]
    fn nil_controller_completes_without_retaining() {
        let request = ViewRequest::default();
        assert_eq!(request.complete(0), None);
        assert_eq!(request.take(), Some(ViewCompletion::NoCustomView));
    }

    #[test]
    fn rejected_background_completion_is_deferred_to_main_release() {
        let request = Arc::new(ViewRequest::default());
        assert_eq!(request.cancel(), None);
        let (sent, received) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let on_main = MainThreadMarker::new().is_some();
            assert!(!on_main, "regression callback must run off main thread");
            release_owned_with(
                request.complete(43),
                on_main,
                |_| panic!("background callback released a controller locally"),
                |raw| assert!(sent.send(raw).is_ok()),
            );
        })
        .join()
        .expect("background callback test thread panicked");
        assert_eq!(received.recv(), Ok(43));

        let mut released = None;
        release_owned_with(
            Some(44),
            true,
            |raw| released = Some(raw),
            |_| panic!("main-thread cleanup was unnecessarily dispatched"),
        );
        assert_eq!(released, Some(44));
    }
}
