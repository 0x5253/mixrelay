//! Synchronous and asynchronous native instance ownership transfer.

use objc2_audio_toolbox::{
    AudioComponent, AudioComponentInstance, AudioComponentInstanceDispose,
    AudioComponentInstanceNew, AudioComponentInstantiate, AudioComponentInstantiationOptions,
};
use std::ptr;
use std::sync::{Arc, Mutex};
use truce_rack_core::error::{Error, Result};

/// Synchronous `AudioComponentInstanceNew` wrapper. Used for AU v2
/// and for AU v3 plugins that don't set `RequiresAsyncInstantiation`.
pub(super) fn instantiate_sync(
    component: AudioComponent,
    info_path: &std::path::Path,
) -> Result<AudioComponentInstance> {
    let mut instance: AudioComponentInstance = ptr::null_mut();
    let status = unsafe {
        AudioComponentInstanceNew(component, ptr::NonNull::new_unchecked(&raw mut instance))
    };
    if status != 0 || instance.is_null() {
        return Err(Error::LoadFailed {
            path: info_path.to_path_buf(),
            reason: format!("AudioComponentInstanceNew failed with OSStatus {status}"),
        });
    }
    Ok(instance)
}

/// Asynchronous instantiation via `AudioComponentInstantiate`,
/// required for AU v3 plugins flagged `RequiresAsyncInstantiation`
/// (sandboxed App-Extension-hosted units). This synchronous scanner
/// path pumps the current run loop until completion or a bounded
/// deadline. Because the native callback can arrive after timeout,
/// shared completion state explicitly transfers or abandons ownership;
/// a late successful instance is disposed by the callback exactly once.
///
/// This remains a synchronous, reentrant run-loop wait. Apple warns
/// against waiting on the main thread because instantiation can deadlock;
/// native `AUv3` loading through this API therefore still needs qualification
/// in the actual host lifecycle.
pub(super) fn instantiate_async(
    component: AudioComponent,
    info_path: &std::path::Path,
) -> Result<AudioComponentInstance> {
    use std::time::{Duration, Instant};

    let completion = Arc::new(AsyncInstantiationCompletion::default());
    let callback_completion = Arc::clone(&completion);
    let deadline = Instant::now() + Duration::from_secs(10);

    // RcBlock heap-allocates the block and refcounts via Apple's
    // blocks runtime; AudioComponentInstantiate retains it for the
    // asynchronous completion. The captured Arc keeps its state alive
    // when the caller returns on timeout and the native callback is late.
    let block = block2::RcBlock::new(move |inst: AudioComponentInstance, status: i32| {
        if let Some(late_instance) = callback_completion.complete(inst as usize, status, deadline) {
            // SAFETY: the completion callback transfers each abandoned instance here once;
            // AudioComponentInstanceDispose has no caller-thread restriction in Apple's SDK.
            unsafe { dispose_instance(late_instance) };
        }
    });
    unsafe {
        AudioComponentInstantiate(
            component,
            AudioComponentInstantiationOptions::empty(),
            &block,
        );
    }

    // Pump the caller's current run loop in short slices so its reentrant
    // callbacks and queued work can progress during this legacy sync wait.
    // Completion is documented to run on an arbitrary thread and does
    // not depend on the callback being dispatched onto this run loop.
    loop {
        if Instant::now() >= deadline {
            // This mutex transition races atomically with callback completion. If completion
            // won, abandon takes its pointer for disposal; if timeout won, the callback will
            // dispose any later success. Exactly one side owns every non-null instance.
            if let Some(raw) = completion.abandon() {
                unsafe { dispose_instance(raw) };
            }
            return Err(Error::LoadFailed {
                path: info_path.to_path_buf(),
                reason: "AudioComponentInstantiate timed out after 10s".into(),
            });
        }
        if let Some((raw, status)) = completion.take_completed() {
            if status != 0 || raw == 0 {
                // A failed native completion may still supply a non-null instance.
                // Ownership was transferred to this thread, so release it here.
                unsafe { dispose_instance(raw) };
                return Err(Error::LoadFailed {
                    path: info_path.to_path_buf(),
                    reason: format!("AudioComponentInstantiate failed with OSStatus {status}"),
                });
            }
            return Ok(raw as AudioComponentInstance);
        }
        let mode = unsafe { objc2_core_foundation::kCFRunLoopDefaultMode };
        objc2_core_foundation::CFRunLoop::run_in_mode(mode, 0.05, true);
    }
}

/// Ownership state shared with the one-shot native completion callback.
#[derive(Default)]
pub(super) struct AsyncInstantiationCompletion {
    state: Mutex<AsyncInstantiationState>,
}

#[derive(Clone, Copy, Default)]
pub(super) enum AsyncInstantiationState {
    #[default]
    Pending,
    Completed {
        instance: usize,
        status: i32,
    },
    Abandoned,
    /// The callback arrived after abandonment and its instance was handed off for disposal.
    LateCompletion {
        instance: usize,
    },
    /// The waiting caller took ownership of a completed instance (including failed completions).
    Taken {
        instance: usize,
    },
}

impl AsyncInstantiationCompletion {
    /// Records a native completion. A returned nonzero pointer belongs to the caller for
    /// disposal because the timeout path had already abandoned the operation.
    pub(super) fn complete(
        &self,
        instance: usize,
        status: i32,
        deadline: std::time::Instant,
    ) -> Option<usize> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *state {
            AsyncInstantiationState::Pending => {
                if std::time::Instant::now() < deadline {
                    *state = AsyncInstantiationState::Completed { instance, status };
                    None
                } else {
                    *state = AsyncInstantiationState::LateCompletion { instance };
                    (instance != 0).then_some(instance)
                }
            }
            AsyncInstantiationState::Abandoned => {
                *state = AsyncInstantiationState::LateCompletion { instance };
                (instance != 0).then_some(instance)
            }
            AsyncInstantiationState::LateCompletion { instance: prior }
            | AsyncInstantiationState::Taken { instance: prior }
            | AsyncInstantiationState::Completed {
                instance: prior, ..
            } => (instance != 0 && instance != prior).then_some(instance),
        }
    }

    /// Transfers a completed result to the waiting caller at most once.
    pub(super) fn take_completed(&self) -> Option<(usize, i32)> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let AsyncInstantiationState::Completed { instance, status } = *state else {
            return None;
        };
        *state = AsyncInstantiationState::Taken { instance };
        Some((instance, status))
    }

    /// Abandons a pending request or transfers an already-completed instance to timeout cleanup.
    pub(super) fn abandon(&self) -> Option<usize> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *state {
            AsyncInstantiationState::Pending => {
                *state = AsyncInstantiationState::Abandoned;
                None
            }
            AsyncInstantiationState::Completed { instance, .. } => {
                *state = AsyncInstantiationState::LateCompletion { instance };
                (instance != 0).then_some(instance)
            }
            AsyncInstantiationState::Abandoned
            | AsyncInstantiationState::LateCompletion { .. }
            | AsyncInstantiationState::Taken { .. } => None,
        }
    }
}

/// Dispose a non-null AU instance returned by an asynchronous completion.
///
/// # Safety
/// `instance` must be a live instance whose ownership has been transferred to this caller.
pub(super) unsafe fn dispose_instance(instance: usize) {
    if instance != 0 {
        let instance = instance as AudioComponentInstance;
        // SAFETY: upheld by the caller's exclusive ownership of this completed instance.
        unsafe { AudioComponentInstanceDispose(instance) };
    }
}
