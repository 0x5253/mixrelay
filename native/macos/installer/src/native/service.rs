//! Native `ServiceLifecycle` implementation composed with the fixed transaction core.
//!
//! New code passes Launch verification before registration. Rollback reactivates
//! the previous Driver, engine, HAL and remover only after Ownership verification
//! and equality to the preflight file and launchd snapshots.

use std::{fs, path::PathBuf};

use crate::{
    FilesystemLayout, FilesystemStaged, HAL_PATH, LAUNCHD_PATH, LaunchdConfiguration, NativeError,
    OwnedTarget, ReleasePayloadVerifier, ServiceLifecycle, SigningTeam, TransactionRequest,
};

use super::{
    account::ResolvedServiceAccount,
    audio_probe::{check_coreaudio_response, wait_for_hal_absence, wait_for_hal_presence},
    hal_reload,
    launchctl::{self, RegistrationState},
    trust::{require_lifecycle_storage, require_root, validate_destination_ancestors},
};

mod engine;
mod payload;
mod runtime;

use payload::{installed_hal_version, read_bounded_file};
use runtime::{
    activate_driver, activate_restored_driver, capture_service_process, wait_for_driver_ready,
    wait_for_service_exit,
};

/// The actual macOS `ServiceLifecycle` boundary for the coordinated transaction.
pub struct NativeServiceLifecycle {
    team: SigningTeam,
    account: ResolvedServiceAccount,
    verifier: ReleasePayloadVerifier,
    registration_after_drain: Option<RegistrationState>,
    delete_saved_settings: bool,
    coreaudio_reload_approved: bool,
    coreaudio_reload_attempted: bool,
    hal_hosts: Option<hal_reload::CapturedHosts>,
    hal_mapping_paths: Vec<PathBuf>,
    pending_user_data_cleanup: Option<super::user_data::PendingCleanup>,
    operation: crate::OperationKind,
}

impl NativeServiceLifecycle {
    /// Creates an adapter bound to one Team ID and already OS-resolved account.
    #[must_use]
    pub fn new(team: SigningTeam, account: ResolvedServiceAccount) -> Self {
        let verifier = ReleasePayloadVerifier::new(team.clone(), account.account().clone());
        Self {
            team,
            account,
            verifier,
            registration_after_drain: None,
            delete_saved_settings: false,
            coreaudio_reload_approved: false,
            coreaudio_reload_attempted: false,
            hal_hosts: None,
            hal_mapping_paths: vec![PathBuf::from(HAL_PATH)],
            pending_user_data_cleanup: None,
            operation: crate::OperationKind::Remove,
        }
    }

    /// Records explicit administrator consent to interrupt audio during HAL install/remove.
    /// Without consent, preflight fails before lease acquisition or file/service mutation.
    #[must_use]
    pub const fn with_coreaudio_reload_approval(mut self, approved: bool) -> Self {
        self.coreaudio_reload_approved = approved;
        self
    }

    /// Selects optional Driver settings cleanup alongside automatic management UI cleanup.
    /// Graph App data and authentication are preserved.
    ///
    /// A cleanup failure triggers transaction rollback, which attempts to restore
    /// the installed account plist for a subsequent removal attempt.
    #[must_use]
    pub const fn with_saved_settings_removal(mut self, selected: bool) -> Self {
        self.delete_saved_settings = selected;
        self
    }

    fn verify_service_account(&self) -> Result<(), NativeError> {
        let current = super::account::resolve_service_account(self.account.account())?;
        if current != self.account {
            return Err(NativeError::Account(
                "selected passwd/group/home identity changed during installer transaction"
                    .to_owned(),
            ));
        }
        Ok(())
    }

    fn require_reload_approval(&self) -> Result<(), NativeError> {
        require_reload_approval(self.coreaudio_reload_approved)
    }

    fn reload_coreaudio(&mut self) -> Result<(), NativeError> {
        // Even a failed reload command may have killed the old server. Recovery must
        // reload the restored HAL in that case, not only after command success.
        self.coreaudio_reload_attempted = true;
        launchctl::reload_coreaudio()
    }

    fn current_registration() -> Result<RegistrationState, NativeError> {
        launchctl::inspect_registration()
    }

    fn ensure_idle(state: &RegistrationState) -> Result<(), NativeError> {
        if matches!(state, RegistrationState::Loaded { pid: Some(pid) } if *pid != 0) {
            return Err(NativeError::LaunchdState(
                "the exact MixRelay launchd job is still running",
            ));
        }
        Ok(())
    }

    fn bootstrap_restored_job(
        &mut self,
        expected: &LaunchdConfiguration,
    ) -> Result<(), NativeError> {
        let layout = FilesystemLayout::new("/")?;
        let actual = self
            .verify_installed_payload(&layout)?
            .ok_or(NativeError::LaunchdState(
                "preflight registration snapshot has no complete restored payload",
            ))?;
        if &actual != expected {
            return Err(NativeError::Trust {
                path: PathBuf::from(LAUNCHD_PATH),
                reason: "restored payload differs from its preflight snapshot",
            });
        }
        let installed = PathBuf::from(LAUNCHD_PATH);
        let bytes = read_bounded_file(&installed)?;
        let actual = LaunchdConfiguration::validate_installed(&bytes, self.account.account())?;
        if &actual != expected {
            return Err(NativeError::Trust {
                path: installed,
                reason: "restored launchd plist differs from its preflight snapshot",
            });
        }
        let version = installed_hal_version()?;
        activate_restored_driver(
            &mut self.coreaudio_reload_attempted,
            launchctl::bootstrap,
            || {
                hal_reload::reload_and_wait(
                    self.hal_hosts.as_ref(),
                    &self.hal_mapping_paths,
                    || wait_for_driver_ready(&version, self.account.account().home()),
                )
            },
            || wait_for_driver_ready(&version, self.account.account().home()),
        )
    }
}

impl ServiceLifecycle for NativeServiceLifecycle {
    type Snapshot = NativeServiceSnapshot;
    type Registration = NativeServiceRegistration;
    type Error = NativeError;

    fn inspect(
        &mut self,
        request: &TransactionRequest,
        layout: &FilesystemLayout,
    ) -> Result<Self::Snapshot, Self::Error> {
        self.require_reload_approval()?;
        require_root()?;
        self.verify_service_account()?;
        validate_destination_ancestors(layout.root())?;
        require_lifecycle_storage(&self.account)?;
        // Refuse to start a file transaction while the audio server cannot answer.
        // A blocked CoreAudio lookup is owned and reaped by the probe's deadline.
        check_coreaudio_response()?;
        self.operation = request.kind();
        let installed = self.verify_installed_payload(layout)?;
        let registration = Self::current_registration()?;
        if matches!(request, TransactionRequest::Install(_))
            && installed.is_none()
            && matches!(registration, RegistrationState::Loaded { .. })
        {
            return Err(NativeError::LaunchdState(
                "a service is registered but its complete signed payload set is absent",
            ));
        }
        if let TransactionRequest::Install(sources) = request {
            self.verifier.verify_sources(sources)?;
        }
        let engine_was_registered = recoverable_engine_registration(
            request.kind(),
            engine::registered().map_err(NativeError::UserData)?,
            std::path::Path::new(crate::RUNTIME_LAUNCHD_PATH).is_file(),
        )?;
        self.registration_after_drain = None;
        self.coreaudio_reload_attempted = false;
        // Captured under coordinated installer serialization, before replacing files.
        let hal_was_present = fs::symlink_metadata(HAL_PATH).is_ok();
        self.hal_mapping_paths = vec![layout.target_path(OwnedTarget::Hal)];
        self.hal_hosts = if hal_was_present {
            hal_reload::capture(&self.hal_mapping_paths)
        } else {
            None
        };
        Ok(NativeServiceSnapshot {
            owned_installation: self.verifier.snapshot_owned(layout)?,
            registration,
            launchd_configuration: installed,
            account: self.account.clone(),
            team: self.team.clone(),
            engine_was_registered,
            hal_was_present,
            remover_was_present: fs::symlink_metadata(crate::REMOVAL_LAUNCHD_PATH).is_ok()
                && fs::symlink_metadata(crate::UNINSTALLER_PATH).is_ok(),
        })
    }

    fn validate_stage(
        &mut self,
        request: &TransactionRequest,
        staged: &FilesystemStaged,
    ) -> Result<(), Self::Error> {
        self.require_reload_approval()?;
        require_root()?;
        // Use the filesystem transaction's exact destination, never a backup glob.
        self.hal_mapping_paths
            .push(staged.backup_path(OwnedTarget::Hal));
        if matches!(request, TransactionRequest::Remove) {
            self.verify_service_account()?;
            // The embedded executable is still at its verified original path here;
            // commit moves the remover app into private rollback storage.
            self.pending_user_data_cleanup = Some(
                super::user_data::prepare_for_account(&self.account, self.delete_saved_settings)
                    .map_err(NativeError::UserData)?,
            );
            return Ok(());
        }
        let definition = self.verifier.verify_staged(staged)?;
        Self::write_bound_plist(staged, &definition)?;
        self.write_runtime_plist(staged)
    }

    fn drain(&mut self, snapshot: &Self::Snapshot) -> Result<(), Self::Error> {
        self.require_reload_approval()?;
        require_root()?;
        self.verify_service_account()?;
        validate_destination_ancestors(&PathBuf::from("/"))?;
        require_lifecycle_storage(&self.account)?;
        let current = Self::current_registration()?;
        if current != snapshot.registration {
            return Err(NativeError::LaunchdState(
                "registration changed after serialized preflight",
            ));
        }
        if matches!(current, RegistrationState::Loaded { .. }) {
            let process = capture_service_process(&current)?;
            launchctl::bootout()?;
            wait_for_service_exit(process)?;
            if Self::current_registration()? != RegistrationState::Absent {
                return Err(NativeError::LaunchdState(
                    "Driver bootout returned but the verified service remains registered",
                ));
            }
        }
        if self.operation == crate::OperationKind::Install {
            crate::removal::unregister_helper().map_err(NativeError::UserData)?;
        }
        engine::drain().map_err(NativeError::UserData)?;
        self.registration_after_drain = Some(RegistrationState::Absent);
        Ok(())
    }

    fn register(
        &mut self,
        request: &TransactionRequest,
    ) -> Result<Self::Registration, Self::Error> {
        self.require_reload_approval()?;
        require_root()?;
        self.verify_service_account()?;
        let layout = FilesystemLayout::new("/")?;
        validate_destination_ancestors(layout.root())?;
        require_lifecycle_storage(&self.account)?;
        if matches!(request, TransactionRequest::Install(_)) {
            super::trust::validate_engine_directory(layout.root(), true)?;
            self.verifier
                .verify_installed(&layout, &OwnedTarget::all())?;
        }
        let installed = self.verify_installed_payload(&layout)?;
        match request {
            TransactionRequest::Install(_) if installed.is_none() => {
                return Err(NativeError::Trust {
                    path: PathBuf::from("/"),
                    reason: "committed install payload is absent or incomplete",
                });
            }
            TransactionRequest::Remove
                if OwnedTarget::all()
                    .iter()
                    .any(|target| fs::symlink_metadata(target.path()).is_ok()) =>
            {
                return Err(NativeError::Trust {
                    path: PathBuf::from("/"),
                    reason: "remove commit left an owned payload behind",
                });
            }
            _ => {}
        }
        let expected = self
            .registration_after_drain
            .take()
            .ok_or(NativeError::LaunchdState(
                "registration was attempted without a successful drain",
            ))?;
        let current = Self::current_registration()?;
        if current != expected {
            return Err(NativeError::LaunchdState(
                "registration changed between drain and commit",
            ));
        }
        Self::ensure_idle(&current)?;
        match request {
            TransactionRequest::Install(_) => {
                if std::path::Path::new(crate::RUNTIME_LAUNCHD_PATH).exists() {
                    engine::register().map_err(NativeError::UserData)?;
                }
                let version = installed_hal_version()?;
                activate_driver(
                    &mut self.coreaudio_reload_attempted,
                    launchctl::bootstrap,
                    || {
                        hal_reload::reload_and_wait(
                            self.hal_hosts.as_ref(),
                            &self.hal_mapping_paths,
                            || wait_for_driver_ready(&version, self.account.account().home()),
                        )
                    },
                )?;
                if fs::symlink_metadata(crate::REMOVAL_LAUNCHD_PATH).is_ok() {
                    crate::removal::register_helper(&self.team).map_err(NativeError::UserData)?;
                }
                Ok(NativeServiceRegistration)
            }
            TransactionRequest::Remove => {
                self.reload_coreaudio()?;
                wait_for_hal_absence()?;
                crate::removal::remove_right().map_err(NativeError::UserData)?;
                self.pending_user_data_cleanup
                    .take()
                    .ok_or_else(|| {
                        NativeError::UserData(anyhow::anyhow!("removal cleanup was not prepared"))
                    })?
                    .commit()
                    .map_err(NativeError::UserData)?;
                Ok(NativeServiceRegistration)
            }
        }
    }

    fn prepare_restore(&mut self, snapshot: &Self::Snapshot) -> Result<(), Self::Error> {
        drop(self.pending_user_data_cleanup.take());
        engine::drain().map_err(NativeError::UserData)?;
        if self.operation == crate::OperationKind::Install {
            crate::removal::unregister_helper().map_err(NativeError::UserData)?;
        }
        self.require_reload_approval()?;
        require_root()?;
        if snapshot.account != self.account || snapshot.team != self.team {
            return Err(NativeError::Trust {
                path: PathBuf::from("/"),
                reason: "service rollback snapshot belongs to another account or publisher",
            });
        }
        self.verify_service_account()?;
        let layout = FilesystemLayout::new("/")?;
        validate_destination_ancestors(layout.root())?;
        require_lifecycle_storage(&self.account)?;
        let current = Self::current_registration()?;
        if matches!(current, RegistrationState::Loaded { .. }) {
            // A new Driver may have started before readiness failed. Revalidate
            // its complete signed on-disk identity before stopping the exact job.
            let installed = self.verify_installed_payload(&layout)?;
            if self.operation == crate::OperationKind::Install && installed.is_none() {
                return Err(NativeError::LaunchdState(
                    "cannot drain a registered Driver without its verified payload",
                ));
            }
            let process = capture_service_process(&current)?;
            launchctl::bootout()?;
            wait_for_service_exit(process)?;
        }
        if Self::current_registration()? != RegistrationState::Absent {
            return Err(NativeError::LaunchdState(
                "rollback cannot restore files while the Driver remains registered",
            ));
        }
        // The replacement HAL may now be hosted by a different generation. Capture
        // before filesystem rollback removes it/restores the original bundle. An
        // unresolved deleted-vnode mapping after rollback safely selects fallback.
        self.hal_hosts = if self.coreaudio_reload_attempted && snapshot.hal_was_present {
            hal_reload::capture(&self.hal_mapping_paths)
        } else {
            None
        };
        Ok(())
    }

    fn restore(&mut self, snapshot: &Self::Snapshot) -> Result<(), Self::Error> {
        self.require_reload_approval()?;
        require_root()?;
        if snapshot.account != self.account || snapshot.team != self.team {
            return Err(NativeError::Trust {
                path: PathBuf::from("/"),
                reason: "service rollback snapshot belongs to another account or publisher",
            });
        }
        self.verify_service_account()?;
        validate_destination_ancestors(&PathBuf::from("/"))?;
        require_lifecycle_storage(&self.account)?;
        let layout = FilesystemLayout::new("/")?;
        let restored = self.verify_installed_payload(&layout)?;
        if restored != snapshot.launchd_configuration {
            return Err(NativeError::Trust {
                path: PathBuf::from(LAUNCHD_PATH),
                reason: "restored payload differs from its preflight snapshot",
            });
        }
        if Self::current_registration()? != RegistrationState::Absent {
            return Err(NativeError::LaunchdState(
                "registration changed after rollback drain",
            ));
        }
        self.verifier
            .verify_restored(&layout, &[], &snapshot.owned_installation)?;
        if snapshot.remover_was_present {
            crate::removal::restore_helper(
                &self.verifier,
                &layout,
                &snapshot.owned_installation,
                &self.team,
            )
            .map_err(NativeError::UserData)?;
        } else if self.operation == crate::OperationKind::Install {
            crate::removal::remove_right().map_err(NativeError::UserData)?;
        }
        if snapshot.engine_was_registered {
            engine::register().map_err(NativeError::UserData)?;
        }
        match &snapshot.registration {
            RegistrationState::Absent => {
                if self.coreaudio_reload_attempted {
                    if snapshot.hal_was_present {
                        hal_reload::reload_and_wait(
                            self.hal_hosts.as_ref(),
                            &self.hal_mapping_paths,
                            wait_for_hal_presence,
                        )?;
                    } else {
                        launchctl::reload_coreaudio()?;
                        wait_for_hal_absence()?;
                    }
                }
                Ok(())
            }
            RegistrationState::Loaded { .. } => {
                let expected =
                    snapshot
                        .launchd_configuration
                        .as_ref()
                        .ok_or(NativeError::LaunchdState(
                            "preflight registration snapshot has no validated installed plist",
                        ))?;
                self.bootstrap_restored_job(expected)
            }
        }
    }
}

/// Immutable platform snapshot used to restore launchd registration after file rollback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeServiceSnapshot {
    owned_installation: crate::OwnedInstallationSnapshot,
    registration: RegistrationState,
    launchd_configuration: Option<LaunchdConfiguration>,
    account: ResolvedServiceAccount,
    team: SigningTeam,
    engine_was_registered: bool,
    hal_was_present: bool,
    remover_was_present: bool,
}

/// Receipt of a completed native launchd registration change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeServiceRegistration;

fn require_reload_approval(approved: bool) -> Result<(), NativeError> {
    if !approved {
        return Err(NativeError::CoreAudioReloadApprovalRequired);
    }
    Ok(())
}

// Removal can drain an exact-identity job even after its definition was deleted.
// Rollback can re-register it only when the original definition existed.
fn recoverable_engine_registration(
    operation: crate::OperationKind,
    registered: bool,
    definition_present: bool,
) -> Result<bool, NativeError> {
    if operation == crate::OperationKind::Install && registered && !definition_present {
        return Err(NativeError::LaunchdState(
            "audio engine registration has no validated owned definition",
        ));
    }
    Ok(registered && definition_present)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removal_accepts_a_deleted_engine_definition_without_inventing_rollback_payload() {
        assert!(
            !recoverable_engine_registration(crate::OperationKind::Remove, true, false).unwrap()
        );
        assert!(
            recoverable_engine_registration(crate::OperationKind::Install, true, false).is_err()
        );
        assert!(recoverable_engine_registration(crate::OperationKind::Remove, true, true).unwrap());
    }
}
