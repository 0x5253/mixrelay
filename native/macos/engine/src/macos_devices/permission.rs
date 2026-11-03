//! Obtain first-use consent before preparing native input and classify readiness failures.
use mixrelay_audio_io::HostAudioBackendError;
use std::sync::mpsc::RecvTimeoutError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InputPermission {
    Pending,
    Denied,
    Authorized,
    /// This session relies on the `CoreAudio` user-session consent broker.
    Brokered,
    Unknown,
}

pub(super) fn current() -> InputPermission {
    unsafe extern "C" {
        fn mr_microphone_authorization_status() -> std::ffi::c_long;
    }
    // SAFETY: the shim takes no pointers and only reads this process's public
    // AVFoundation authorization status. It neither requests nor changes access.
    match unsafe { mr_microphone_authorization_status() } {
        0 => InputPermission::Pending,
        1 | 2 => InputPermission::Denied,
        3 => InputPermission::Authorized,
        4 => InputPermission::Brokered,
        _ => InputPermission::Unknown,
    }
}

pub(super) fn request() {
    unsafe extern "C" {
        fn mr_microphone_request_authorization();
    }
    // SAFETY: the shim takes no pointers, retains no Rust state and requests access
    // asynchronously through AVFoundation. The engine owns the input authorization.
    unsafe { mr_microphone_request_authorization() };
}

// Two minutes lets a user read the system prompt while bounding graph preparation.
// A control request returns an operation receipt while this decision is pending.
const CONSENT_POLLS: usize = 1_200;
pub(super) const CONSENT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

pub(super) fn authorize(host: &dyn super::host::Host) -> Result<(), HostAudioBackendError> {
    let initial = host.input_permission();
    host.set_permission_waiting(initial == InputPermission::Pending);
    let result = wait_for_decision(
        initial,
        || host.request_input_permission(),
        || host.poll_input_permission(),
    );
    host.set_permission_waiting(false);
    result
}

fn wait_for_decision(
    mut permission: InputPermission,
    request: impl FnOnce(),
    mut observe: impl FnMut() -> InputPermission,
) -> Result<(), HostAudioBackendError> {
    if permission == InputPermission::Pending {
        request();
        for _ in 0..CONSENT_POLLS {
            permission = observe();
            if permission != InputPermission::Pending {
                break;
            }
        }
    }
    match permission {
        InputPermission::Pending => Err(HostAudioBackendError::PermissionPending {
            capability: "microphone".into(),
        }),
        InputPermission::Denied => Err(HostAudioBackendError::PermissionDenied {
            capability: "microphone".into(),
        }),
        InputPermission::Authorized | InputPermission::Brokered | InputPermission::Unknown => {
            Ok(())
        }
    }
}

pub(super) fn wait_for_native(
    host: &dyn super::host::Host,
    permission: Option<InputPermission>,
    receive: impl FnOnce(std::time::Duration) -> Result<Result<(), String>, RecvTimeoutError>,
) -> Result<(), HostAudioBackendError> {
    let brokered = permission == Some(InputPermission::Brokered);
    // A daemon cannot preflight the user's TCC grant through AVFoundation.
    // Its actual IOProc registration requests consent through CoreAudio instead.
    host.set_permission_waiting(brokered);
    let timeout = std::time::Duration::from_secs(if brokered { 120 } else { 5 });
    let result = receive(timeout);
    host.set_permission_waiting(false);
    initialization(result, permission, || host.input_permission())
}

pub(super) fn initialization(
    result: Result<Result<(), String>, RecvTimeoutError>,
    permission: Option<InputPermission>,
    latest: impl FnOnce() -> InputPermission,
) -> Result<(), HostAudioBackendError> {
    if matches!(result, Ok(Ok(()))) {
        return Ok(());
    }
    // First-use consent can consume the readiness budget even when approval
    // arrives just before expiry. A new denial still takes precedence.
    let permission = permission.map(|initial| match latest() {
        InputPermission::Authorized if initial == InputPermission::Pending => {
            InputPermission::Pending
        }
        current => current,
    });
    match result {
        Err(RecvTimeoutError::Timeout)
            if matches!(
                permission,
                Some(InputPermission::Pending | InputPermission::Brokered)
            ) =>
        {
            Err(HostAudioBackendError::PermissionPending {
                capability: "microphone".into(),
            })
        }
        error if permission == Some(InputPermission::Denied) => {
            tracing::warn!(
                ?error,
                "CoreAudio initialization failed with microphone access denied"
            );
            Err(HostAudioBackendError::PermissionDenied {
                capability: "microphone".into(),
            })
        }
        error => Err(super::backend_error(format!(
            "CoreAudio device initialization failed: {error:?}"
        ))),
    }
}

#[cfg(test)]
mod tests;
