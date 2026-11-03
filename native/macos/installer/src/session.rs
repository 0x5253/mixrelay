//! Preserve only the apps this authorized transaction actually closes.

#[cfg(target_os = "macos")]
use crate::APP_PATH;
use crate::OwnedTarget;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionApp {
    /// The `MixRelay` editor window process.
    Graph,
    /// The background audio runtime role of the same `MixRelay` bundle.
    UnifiedRuntime,
}

impl SessionApp {
    pub const ALL: [Self; 2] = [Self::Graph, Self::UnifiedRuntime];

    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn matches_command(self, command: &str) -> bool {
        let executable = format!("{APP_PATH}/Contents/MacOS/mixrelay-desktop");
        match self {
            Self::UnifiedRuntime => command == format!("{executable} --driver-runtime"),
            Self::Graph => command == executable,
        }
    }

    #[must_use]
    pub const fn quit_action(self) -> &'static str {
        match self {
            Self::Graph => "quit-graph-app",
            Self::UnifiedRuntime => "quit-mixrelay-runtime",
        }
    }

    #[must_use]
    pub const fn restore_action(self) -> &'static str {
        match self {
            Self::Graph => "restore-graph-app",
            Self::UnifiedRuntime => "restore-mixrelay-runtime",
        }
    }

    /// Installed targets the app needs to run. Others may be missing when it is reopened.
    const fn dependencies(self) -> &'static [OwnedTarget] {
        match self {
            Self::Graph => &[
                OwnedTarget::App,
                OwnedTarget::RuntimeEngine,
                OwnedTarget::RuntimeLaunchd,
            ],
            Self::UnifiedRuntime => &[
                OwnedTarget::App,
                OwnedTarget::Driver,
                OwnedTarget::Hal,
                OwnedTarget::Launchd,
                OwnedTarget::RuntimeEngine,
                OwnedTarget::RuntimeLaunchd,
            ],
        }
    }
}

/// Installed targets that reopening `apps` needs, in transaction order.
#[must_use]
pub fn dependencies(apps: &[SessionApp]) -> Vec<OwnedTarget> {
    OwnedTarget::all()
        .into_iter()
        .filter(|target| apps.iter().any(|app| app.dependencies().contains(target)))
        .collect()
}

/// An app-session action or restoration failed around the transaction.
#[derive(Debug, thiserror::Error)]
pub enum SessionError<E> {
    #[error("{0}")]
    Action(E),
    #[error("invalid app Quit acknowledgement")]
    InvalidQuitAcknowledgement,
    #[error("update completed but app relaunch failed: {0}")]
    Relaunch(E),
    #[error("{original}; app restoration failed: {restoration}")]
    Recovery { original: Box<Self>, restoration: E },
}

/// Closes the running roles and restores them after an update or recoverable failure.
///
/// # Errors
/// Returns the action or transaction failure, invalid Quit acknowledgement, or
/// relaunch failure. Recovery failures retain the original transaction error.
pub fn run_with_apps_closed<T, E>(
    running: &[SessionApp],
    install: bool,
    mut action: impl FnMut(&'static str) -> Result<String, E>,
    transaction: impl FnOnce() -> Result<T, E>,
    can_recover: impl FnOnce(&[SessionApp]) -> bool,
) -> Result<T, SessionError<E>> {
    let mut closed = Vec::new();
    let result = (|| {
        for app in running {
            // A refused/timed-out Quit can have already closed some processes.
            closed.push(*app);
            let acknowledgement = action(app.quit_action()).map_err(SessionError::Action)?;
            if !matches!(acknowledgement.trim(), "true" | "false") {
                return Err(SessionError::InvalidQuitAcknowledgement);
            }
            if acknowledgement.trim() == "false" {
                closed.pop();
            }
        }
        transaction().map_err(SessionError::Action)
    })();
    if (result.is_ok() && install) || (result.is_err() && can_recover(&closed)) {
        // Restore the runtime before the graph app; attempt both even if one fails.
        let mut restoration_error = None;
        for app in closed.iter().rev() {
            if let Err(error) = action(app.restore_action()) {
                restoration_error.get_or_insert(error);
            }
        }
        if let Some(error) = restoration_error {
            return match result {
                Ok(_) => Err(SessionError::Relaunch(error)),
                Err(original) => Err(SessionError::Recovery {
                    original: Box::new(original),
                    restoration: error,
                }),
            };
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use std::cell::RefCell;

    #[test]
    #[cfg(target_os = "macos")]
    fn session_detection_requires_the_product_folder_and_exact_role() {
        let editor = "/Applications/MixRelay/MixRelay.app/Contents/MacOS/mixrelay-desktop";
        let runtime = format!("{editor} --driver-runtime");
        assert!(SessionApp::Graph.matches_command(editor));
        assert!(SessionApp::UnifiedRuntime.matches_command(&runtime));
        assert!(!SessionApp::Graph.matches_command(&runtime));
        assert!(!SessionApp::UnifiedRuntime.matches_command(editor));
        assert!(
            !SessionApp::Graph
                .matches_command("/Applications/MixRelay.app/Contents/MacOS/mixrelay-desktop")
        );
        assert!(!SessionApp::Graph.matches_command(&format!("{editor} --unexpected")));
    }

    #[test]
    fn update_reopens_runtime_and_editor_only_after_commit() {
        let events = RefCell::new(Vec::new());
        run_with_apps_closed(
            &SessionApp::ALL,
            true,
            |action| {
                events.borrow_mut().push(action);
                Ok::<_, anyhow::Error>("true".into())
            },
            || {
                events.borrow_mut().push("commit");
                Ok(())
            },
            |_| panic!("successful update requires no recovery"),
        )
        .unwrap();
        assert_eq!(
            *events.borrow(),
            [
                "quit-graph-app",
                "quit-mixrelay-runtime",
                "commit",
                "restore-mixrelay-runtime",
                "restore-graph-app"
            ]
        );
    }

    #[test]
    fn first_install_and_stopped_update_launch_nothing() {
        run_with_apps_closed(
            &[],
            true,
            |_| panic!("no running apps"),
            || Ok::<_, anyhow::Error>(()),
            |_| false,
        )
        .unwrap();
    }

    #[test]
    fn app_that_exited_before_quit_is_not_reopened() {
        let mut actions = Vec::new();
        run_with_apps_closed(
            &SessionApp::ALL,
            true,
            |action| {
                actions.push(action);
                Ok::<_, anyhow::Error>(
                    if action == "quit-graph-app" {
                        "false"
                    } else {
                        "true"
                    }
                    .into(),
                )
            },
            || Ok::<_, anyhow::Error>(()),
            |_| false,
        )
        .unwrap();
        assert_eq!(
            actions,
            [
                "quit-graph-app",
                "quit-mixrelay-runtime",
                "restore-mixrelay-runtime"
            ]
        );
    }

    #[test]
    fn successful_removal_never_reopens_apps() {
        let mut actions = Vec::new();
        run_with_apps_closed(
            &[SessionApp::UnifiedRuntime],
            false,
            |action| {
                actions.push(action);
                Ok::<_, anyhow::Error>("true".into())
            },
            || Ok::<_, anyhow::Error>(()),
            |_| panic!("no recovery"),
        )
        .unwrap();
        assert_eq!(actions, ["quit-mixrelay-runtime"]);
    }

    #[test]
    fn failed_update_reopens_only_when_installed_state_can_be_recovered() {
        for recover in [false, true] {
            let mut actions = Vec::new();
            let result: Result<(), SessionError<anyhow::Error>> = run_with_apps_closed(
                &[SessionApp::UnifiedRuntime],
                true,
                |action| {
                    actions.push(action);
                    Ok::<_, anyhow::Error>("true".into())
                },
                || anyhow::bail!("commit failed"),
                |_| recover,
            );
            assert!(result.is_err());
            assert_eq!(actions.len(), if recover { 2 } else { 1 });
        }
    }

    #[test]
    fn failed_repair_checks_only_what_the_closed_apps_need_before_reopening_them() {
        let mut actions = Vec::new();
        let mut checked = None;
        let result: Result<(), SessionError<anyhow::Error>> = run_with_apps_closed(
            &SessionApp::ALL,
            true,
            |action| {
                actions.push(action);
                Ok::<_, anyhow::Error>(
                    if action == "quit-mixrelay-runtime" {
                        "false"
                    } else {
                        "true"
                    }
                    .into(),
                )
            },
            || anyhow::bail!("repair rolled back"),
            |closed| {
                checked = Some(dependencies(closed));
                true
            },
        );
        assert!(result.is_err());
        // Only the editor was closed, so the rolled-back installation needs only what the
        // editor runs on; a missing Driver or remover does not keep it closed.
        assert_eq!(
            checked.unwrap(),
            [
                OwnedTarget::App,
                OwnedTarget::RuntimeEngine,
                OwnedTarget::RuntimeLaunchd
            ]
        );
        assert_eq!(
            actions,
            [
                "quit-graph-app",
                "quit-mixrelay-runtime",
                "restore-graph-app"
            ]
        );
        assert_eq!(
            dependencies(&SessionApp::ALL),
            [
                OwnedTarget::App,
                OwnedTarget::Driver,
                OwnedTarget::Hal,
                OwnedTarget::Launchd,
                OwnedTarget::RuntimeEngine,
                OwnedTarget::RuntimeLaunchd
            ]
        );
    }

    #[test]
    fn quit_failure_does_not_execute_transaction_and_restores_attempted_apps() {
        let mut actions = Vec::new();
        let result: Result<(), SessionError<anyhow::Error>> = run_with_apps_closed(
            &SessionApp::ALL,
            true,
            |action| {
                actions.push(action);
                if action == "quit-mixrelay-runtime" {
                    anyhow::bail!("Quit timed out");
                }
                Ok::<_, anyhow::Error>("true".into())
            },
            || panic!("Quit did not complete"),
            |_| true,
        );
        assert!(result.is_err());
        assert_eq!(
            actions,
            [
                "quit-graph-app",
                "quit-mixrelay-runtime",
                "restore-mixrelay-runtime",
                "restore-graph-app"
            ]
        );
    }

    #[test]
    fn relaunch_failure_does_not_skip_other_app_or_repeat_commit() {
        let events = RefCell::new(Vec::new());
        let result: Result<(), SessionError<anyhow::Error>> = run_with_apps_closed(
            &SessionApp::ALL,
            true,
            |action| {
                events.borrow_mut().push(action);
                if action == "restore-mixrelay-runtime" {
                    anyhow::bail!("launch failed");
                }
                Ok::<_, anyhow::Error>("true".into())
            },
            || {
                events.borrow_mut().push("commit");
                Ok(())
            },
            |_| false,
        );
        assert!(result.unwrap_err().to_string().contains("update completed"));
        assert_eq!(
            *events.borrow(),
            [
                "quit-graph-app",
                "quit-mixrelay-runtime",
                "commit",
                "restore-mixrelay-runtime",
                "restore-graph-app"
            ]
        );
    }
}
