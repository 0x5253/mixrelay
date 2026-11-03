use super::*;

#[test]
fn delayed_first_use_consent_is_distinct_from_device_failure() {
    let failure = || Err(RecvTimeoutError::Timeout);
    assert_eq!(
        initialization(failure(), Some(InputPermission::Pending), || {
            InputPermission::Pending
        }),
        Err(HostAudioBackendError::PermissionPending {
            capability: "microphone".into()
        })
    );
    assert_eq!(
        initialization(failure(), Some(InputPermission::Denied), || {
            InputPermission::Denied
        }),
        Err(HostAudioBackendError::PermissionDenied {
            capability: "microphone".into()
        })
    );
    for permission in [
        None,
        Some(InputPermission::Authorized),
        Some(InputPermission::Unknown),
    ] {
        assert!(matches!(
            initialization(failure(), permission, || permission.unwrap()),
            Err(HostAudioBackendError::BackendFailure { .. })
        ));
    }
}

#[test]
fn first_prompt_decision_is_observed_and_late_approval_does_not_become_a_disconnect() {
    assert_eq!(
        initialization(
            Ok(Err("input access failed".into())),
            Some(InputPermission::Pending),
            || InputPermission::Denied
        ),
        Err(HostAudioBackendError::PermissionDenied {
            capability: "microphone".into()
        })
    );
    assert_eq!(
        initialization(
            Err(RecvTimeoutError::Timeout),
            Some(InputPermission::Pending),
            || InputPermission::Authorized
        ),
        Err(HostAudioBackendError::PermissionPending {
            capability: "microphone".into()
        })
    );
    assert!(matches!(
        initialization(Err(RecvTimeoutError::Timeout), None, || panic!(
            "output-only initialization must not query microphone consent"
        )),
        Err(HostAudioBackendError::BackendFailure { .. })
    ));
}

#[test]
fn consent_does_not_override_native_readiness_or_a_failed_worker() {
    assert_eq!(
        initialization(Ok(Ok(())), Some(InputPermission::Pending), || panic!(
            "ready inputs do not need another query"
        )),
        Ok(())
    );
    assert_eq!(
        initialization(Ok(Ok(())), Some(InputPermission::Denied), || panic!(
            "ready inputs do not need another query"
        )),
        Ok(())
    );
    for failure in [
        Err(RecvTimeoutError::Disconnected),
        Ok(Err("native device disappeared".into())),
    ] {
        assert!(matches!(
            initialization(failure, Some(InputPermission::Pending), || {
                InputPermission::Pending
            }),
            Err(HostAudioBackendError::BackendFailure { .. })
        ));
    }
}

#[test]
fn first_use_waits_through_the_prompt_and_continues_without_a_second_request() {
    let mut decisions = [
        InputPermission::Pending,
        InputPermission::Pending,
        InputPermission::Authorized,
    ]
    .into_iter();
    let mut requested = 0;
    assert_eq!(
        wait_for_decision(
            InputPermission::Pending,
            || requested += 1,
            || decisions.next().unwrap()
        ),
        Ok(())
    );
    assert_eq!(requested, 1);
    assert!(decisions.next().is_none());
}

#[test]
fn brokered_consent_requires_native_readiness_and_keeps_ordinary_device_limits() {
    use crate::macos_devices::{host::Host as _, simulated::SimulatedHost};
    use std::time::Duration;
    let host = SimulatedHost::new();
    host.consent_decisions([InputPermission::Brokered]);
    assert_eq!(authorize(&*host), Ok(()));
    assert_eq!(host.consent_requests(), 0);
    assert_eq!(
        wait_for_native(&*host, Some(InputPermission::Brokered), |timeout| {
            assert_eq!(timeout, Duration::from_mins(2));
            assert!(host.permission_waiting());
            Err(RecvTimeoutError::Timeout)
        }),
        Err(HostAudioBackendError::PermissionPending {
            capability: "microphone".into()
        })
    );
    assert!(!host.permission_waiting());
    assert_eq!(
        wait_for_native(&*host, Some(InputPermission::Brokered), |_| Ok(Ok(()))),
        Ok(())
    );
    assert!(!host.permission_waiting());
    assert!(matches!(
        wait_for_native(&*host, Some(InputPermission::Brokered), |_| {
            Ok(Err("native input was rejected".into()))
        }),
        Err(HostAudioBackendError::BackendFailure { .. })
    ));
    for permission in [
        None,
        Some(InputPermission::Authorized),
        Some(InputPermission::Unknown),
    ] {
        host.consent_decisions([permission.unwrap_or(InputPermission::Authorized)]);
        assert!(matches!(
            wait_for_native(&*host, permission, |timeout| {
                assert_eq!(timeout, Duration::from_secs(5));
                assert!(!host.permission_waiting());
                Err(RecvTimeoutError::Timeout)
            }),
            Err(HostAudioBackendError::BackendFailure { .. })
        ));
    }
}

#[test]
fn denied_consent_and_an_unanswered_prompt_are_bounded_and_distinct() {
    assert!(matches!(
        wait_for_decision(InputPermission::Pending, || {}, || InputPermission::Denied),
        Err(HostAudioBackendError::PermissionDenied { .. })
    ));
    let mut polls = 0;
    assert!(matches!(
        wait_for_decision(
            InputPermission::Pending,
            || {},
            || {
                polls += 1;
                InputPermission::Pending
            }
        ),
        Err(HostAudioBackendError::PermissionPending { .. })
    ));
    assert_eq!(polls, CONSENT_POLLS);
    for decision in [
        InputPermission::Denied,
        InputPermission::Authorized,
        InputPermission::Unknown,
    ] {
        let result = wait_for_decision(
            decision,
            || panic!("decided consent does not prompt"),
            || panic!("decided consent does not wait"),
        );
        assert_eq!(result.is_ok(), decision != InputPermission::Denied);
    }
}

#[test]
fn native_input_opens_on_the_original_call_after_first_use_approval() {
    use crate::macos_devices::{device::Device, simulated::SimulatedHost, spec::Spec};
    let host = SimulatedHost::new();
    let uid = host.plug("consent", 2, 48_000, 0.25);
    host.consent_decisions([
        InputPermission::Pending,
        InputPermission::Pending,
        InputPermission::Authorized,
    ]);
    let spec = Spec {
        selector: mixrelay_audio_io::DeviceSelector::PersistentUid { uid },
        input: true,
        channels: 2,
    };
    let device = Device::open(&spec, 48_000, host.clone()).unwrap();
    assert_eq!(host.consent_requests(), 1);
    assert_eq!(host.consent_observations(), vec![true, true]);
    assert!(!super::super::host::Host::permission_waiting(&*host));
    assert!(device.ready());
    assert_eq!(host.registrations(), 1);
}

#[test]
fn denied_input_never_allocates_a_native_registration_and_output_never_prompts() {
    use crate::macos_devices::{device::Device, simulated::SimulatedHost, spec::Spec};
    let host = SimulatedHost::new();
    let uid = host.plug("denied", 2, 48_000, 0.25);
    host.consent_decisions([InputPermission::Pending, InputPermission::Denied]);
    let mut spec = Spec {
        selector: mixrelay_audio_io::DeviceSelector::PersistentUid { uid },
        input: true,
        channels: 2,
    };
    assert!(matches!(
        Device::open(&spec, 48_000, host.clone()),
        Err(HostAudioBackendError::PermissionDenied { .. })
    ));
    assert_eq!(host.registrations(), 0);
    assert_eq!(host.consent_requests(), 1);
    assert_eq!(host.consent_observations(), vec![true]);
    assert!(!super::super::host::Host::permission_waiting(&*host));
    spec.input = false;
    let _output = Device::open(&spec, 48_000, host.clone()).unwrap();
    assert_eq!(host.consent_requests(), 1);
}
