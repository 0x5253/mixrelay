//! Activates the Driver and waits for bounded launchd and readiness transitions.

use super::super::launchctl::{self, RegistrationState};
use crate::NativeError;
use crate::process_exit::ProcessIdentity;
use mixrelay_driver_adapters::DriverClient;
use mixrelay_local_socket::{DRIVER_SOCKET, socket_directory_in_home};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(super) fn activate_driver(
    reload_attempted: &mut bool,
    bootstrap: impl FnOnce() -> Result<(), NativeError>,
    activate: impl FnOnce() -> Result<(), NativeError>,
) -> Result<(), NativeError> {
    bootstrap()?;
    *reload_attempted = true;
    activate()
}

pub(super) fn activate_restored_driver(
    reload_attempted: &mut bool,
    bootstrap: impl FnOnce() -> Result<(), NativeError>,
    activate: impl FnOnce() -> Result<(), NativeError>,
    ready: impl FnOnce() -> Result<(), NativeError>,
) -> Result<(), NativeError> {
    if *reload_attempted {
        activate_driver(reload_attempted, bootstrap, activate)
    } else {
        bootstrap()?;
        ready()
    }
}

/// The service account's Driver socket. The installer runs as root; its own HOME or socket
/// override must not select it.
fn socket_client(home: &Path) -> Result<DriverClient, NativeError> {
    DriverClient::with_socket_path(socket_directory_in_home(home).join(DRIVER_SOCKET))
        .map_err(|error| NativeError::Activation(format!("Driver client: {error}")))
}

struct NativeReadiness {
    client: DriverClient,
}

impl crate::activation::DriverReadiness for NativeReadiness {
    type Error = NativeError;

    fn observe(&mut self) -> Result<crate::activation::DriverObservation, NativeError> {
        let snapshot = self
            .client
            .snapshot_blocking()
            .map_err(|error| NativeError::Activation(format!("Driver snapshot: {error}")))?;
        // Observe launchd after the response so a stale owner cannot satisfy readiness.
        let registration = launchctl::inspect_registration()?;
        let registered_pid = match registration {
            RegistrationState::Loaded { pid } => pid,
            RegistrationState::Absent => None,
        };
        Ok(crate::activation::DriverObservation {
            state: snapshot.state,
            installed_version: snapshot.installed_version,
            owner_process_id: snapshot.owner_process_id,
            registered_pid,
        })
    }
}

pub(super) fn wait_for_driver_ready(version: &str, home: &Path) -> Result<(), NativeError> {
    let mut probe = NativeReadiness {
        client: socket_client(home)?,
    };
    crate::activation::wait_until_ready(version, &mut probe, &mixrelay_time::StdClock::new())
        .map_err(|error| NativeError::Activation(error.to_string()))
}

/// Identifies the loaded Driver process before it is stopped, so the exit wait cannot
/// adopt a process that reuses its PID afterwards.
pub(super) fn capture_service_process(
    state: &RegistrationState,
) -> Result<Option<ProcessIdentity>, NativeError> {
    let RegistrationState::Loaded { pid: Some(pid) } = state else {
        return Ok(None);
    };
    let pid = i32::try_from(*pid).map_err(|_| NativeError::LaunchdState("invalid Driver PID"))?;
    ProcessIdentity::capture(pid)
        .map_err(|_| NativeError::LaunchdState("Driver process state is unavailable"))
}

pub(super) fn wait_for_service_exit(process: Option<ProcessIdentity>) -> Result<(), NativeError> {
    let Some(process) = process else {
        return Ok(());
    };
    let remaining = crate::process_exit::wait_for_exit(
        &[process],
        Instant::now() + Duration::from_secs(5),
        std::thread::sleep,
    )
    .map_err(|_| NativeError::LaunchdState("Driver process state is unavailable"))?;
    if remaining.is_empty() {
        Ok(())
    } else {
        Err(NativeError::LaunchdState(
            "Driver process did not drain after bootout",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::hal_reload::{
        HostIdentity, HostProcesses, capture_hosts, reload_with_readiness,
    };
    use super::super::require_reload_approval;
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn approval_is_required_even_for_an_otherwise_valid_transaction() {
        assert!(matches!(
            require_reload_approval(false),
            Err(NativeError::CoreAudioReloadApprovalRequired)
        ));
        assert!(require_reload_approval(true).is_ok());
    }

    #[test]
    fn activation_bootstraps_then_reloads_once_then_checks_readiness() {
        let events = RefCell::new(Vec::new());
        let mut attempted = false;
        activate_driver(
            &mut attempted,
            || {
                events.borrow_mut().push("bootstrap");
                Ok(())
            },
            || {
                events.borrow_mut().push("reload");
                events.borrow_mut().push("ready");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(*events.borrow(), ["bootstrap", "reload", "ready"]);
        assert!(attempted);
    }

    #[test]
    fn failed_bootstrap_never_reloads_and_failed_reload_still_requires_runtime_recovery() {
        for failed_operation in ["bootstrap", "reload", "ready"] {
            let events = RefCell::new(Vec::new());
            let mut attempted = false;
            let action = |operation| {
                events.borrow_mut().push(operation);
                if failed_operation == operation {
                    Err(NativeError::LaunchdState("injected activation failure"))
                } else {
                    Ok(())
                }
            };
            assert!(
                activate_driver(
                    &mut attempted,
                    || action("bootstrap"),
                    || {
                        action("reload")?;
                        action("ready")
                    }
                )
                .is_err()
            );
            let expected = match failed_operation {
                "bootstrap" => vec!["bootstrap"],
                "reload" => vec!["bootstrap", "reload"],
                "ready" => vec!["bootstrap", "reload", "ready"],
                _ => unreachable!(),
            };
            assert_eq!(*events.borrow(), expected);
            assert_eq!(attempted, failed_operation != "bootstrap");
        }
    }

    struct RestoredHosts<'a> {
        loaded: bool,
        events: &'a RefCell<Vec<&'static str>>,
    }

    impl HostProcesses for RestoredHosts<'_> {
        fn candidates(&mut self) -> Result<Vec<u32>, NativeError> {
            self.events.borrow_mut().push("discover");
            Ok(if self.loaded { vec![42] } else { Vec::new() })
        }
        fn identity(
            &mut self,
            _pid: u32,
            _bundles: &[std::path::PathBuf],
        ) -> Result<HostIdentity, NativeError> {
            self.events.borrow_mut().push("validate");
            Ok(HostIdentity {
                started: "restored-host".to_owned(),
                command: "Core Audio Driver (MixRelayHAL.driver)".to_owned(),
            })
        }
        fn signal(&mut self, pid: u32, signal: &'static str) -> Result<(), NativeError> {
            assert_eq!((pid, signal), (42, "-TERM"));
            self.events.borrow_mut().push("term");
            Ok(())
        }
        fn wait_for_exit(&mut self, _pid: u32) -> Result<bool, NativeError> {
            self.events.borrow_mut().push("exit");
            Ok(true)
        }
    }

    #[test]
    fn recovery_prefers_targeted_reload_and_retains_fallback_and_no_reload_paths() {
        for (attempted, loaded, fails_once, expected) in [
            (
                true,
                true,
                false,
                "bootstrap discover validate validate term exit ready",
            ),
            (
                true,
                true,
                true,
                "bootstrap discover validate validate term exit ready full ready",
            ),
            (true, false, false, "bootstrap full ready"),
            (false, true, false, "bootstrap ready"),
        ] {
            let events = RefCell::new(Vec::new());
            let mut hosts = RestoredHosts {
                loaded,
                events: &events,
            };
            let bundles = [std::path::PathBuf::from(crate::HAL_PATH)];
            // Recovery captures hosts before file rollback and activation.
            let captured = attempted
                .then(|| capture_hosts(&mut hosts, &bundles).ok())
                .flatten();
            events.borrow_mut().clear();
            let mut attempted = attempted;
            let mut fail = fails_once;
            let ready = || {
                events.borrow_mut().push("ready");
                if std::mem::take(&mut fail) {
                    Err(NativeError::Activation(
                        "restored Driver not ready".to_owned(),
                    ))
                } else {
                    Ok(())
                }
            };
            activate_restored_driver(
                &mut attempted,
                || {
                    events.borrow_mut().push("bootstrap");
                    Ok(())
                },
                || {
                    reload_with_readiness(
                        captured.as_ref(),
                        &bundles,
                        &mut hosts,
                        || {
                            events.borrow_mut().push("full");
                            Ok(())
                        },
                        ready,
                    )
                },
                || {
                    events.borrow_mut().push("ready");
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(
                *events.borrow(),
                expected.split_whitespace().collect::<Vec<_>>()
            );
        }
    }
}
