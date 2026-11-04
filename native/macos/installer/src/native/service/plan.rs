//! launchd job decisions the lifecycle takes from its preflight snapshot.

use super::super::launchctl::RegistrationState;
use crate::{NativeError, OperationKind};

// Removal can drain an exact-identity job even after its definition was deleted.
// Rollback can re-register it only when the original definition existed.
pub(super) fn recoverable_engine_registration(
    operation: OperationKind,
    registered: bool,
    definition_present: bool,
) -> Result<bool, NativeError> {
    if operation == OperationKind::Install && registered && !definition_present {
        return Err(NativeError::LaunchdState(
            "audio engine registration has no validated owned definition",
        ));
    }
    Ok(registered && definition_present)
}

/// What returning the Driver to its preflight registration needs after file rollback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RegistrationRestore {
    /// The Driver was not registered and Core Audio was never reloaded.
    Nothing,
    /// Core Audio was reloaded and the restored HAL must load again.
    ReloadRestoredHal,
    /// Core Audio was reloaded although no HAL existed before; it must unload again.
    ReloadWithoutHal,
    /// The Driver was registered before; bootstrap the restored job.
    BootstrapRestoredJob,
}

impl RegistrationRestore {
    pub(super) fn plan(
        registration: &RegistrationState,
        reload_attempted: bool,
        hal_was_present: bool,
    ) -> Self {
        match registration {
            RegistrationState::Loaded { .. } => Self::BootstrapRestoredJob,
            RegistrationState::Absent if !reload_attempted => Self::Nothing,
            RegistrationState::Absent if hal_was_present => Self::ReloadRestoredHal,
            RegistrationState::Absent => Self::ReloadWithoutHal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_accepts_a_deleted_engine_definition_without_inventing_rollback_payload() {
        assert!(!recoverable_engine_registration(OperationKind::Remove, true, false).unwrap());
        assert!(recoverable_engine_registration(OperationKind::Install, true, false).is_err());
        assert!(recoverable_engine_registration(OperationKind::Remove, true, true).unwrap());
        assert!(!recoverable_engine_registration(OperationKind::Install, false, true).unwrap());
    }

    #[test]
    fn a_registered_driver_is_bootstrapped_whatever_happened_to_core_audio() {
        for pid in [None, Some(42)] {
            for (reloaded, hal) in [(false, false), (true, false), (false, true), (true, true)] {
                assert_eq!(
                    RegistrationRestore::plan(&RegistrationState::Loaded { pid }, reloaded, hal),
                    RegistrationRestore::BootstrapRestoredJob
                );
            }
        }
    }

    #[test]
    fn an_unregistered_driver_reloads_core_audio_only_after_an_earlier_reload() {
        let absent = RegistrationState::Absent;
        assert_eq!(
            RegistrationRestore::plan(&absent, false, true),
            RegistrationRestore::Nothing
        );
        assert_eq!(
            RegistrationRestore::plan(&absent, false, false),
            RegistrationRestore::Nothing
        );
        assert_eq!(
            RegistrationRestore::plan(&absent, true, true),
            RegistrationRestore::ReloadRestoredHal
        );
        assert_eq!(
            RegistrationRestore::plan(&absent, true, false),
            RegistrationRestore::ReloadWithoutHal
        );
    }
}
