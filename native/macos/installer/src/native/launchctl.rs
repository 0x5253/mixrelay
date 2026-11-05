//! Fixed launchctl commands for the product's system launchd jobs.

mod atoms;
mod status;

pub(crate) use status::RegistrationState;
use status::{Invocation, ServiceIdentity};

use super::{NativeError, command};
use crate::{
    LAUNCHD_PATH, REMOVAL_HELPER_PATH, REMOVAL_LAUNCHD_PATH, RUNTIME_ENGINE_PATH,
    RUNTIME_LAUNCHD_PATH, contract::DRIVER_PATH,
};

pub(super) const PROGRAM: &str = "/bin/launchctl";

/// One system launchd job: its runtime identity and the plist that registers it.
pub(crate) struct Job {
    identity: ServiceIdentity,
    plist: &'static str,
}

/// The Driver service job.
pub(crate) const DRIVER: Job = Job {
    identity: ServiceIdentity {
        label: "io.mixrelay.audio.driver",
        program: DRIVER_PATH,
        invocation: Invocation::Executable,
    },
    plist: LAUNCHD_PATH,
};

/// The on-demand audio engine job.
pub(crate) const ENGINE: Job = Job {
    identity: ServiceIdentity {
        label: "io.mixrelay.audio.engine",
        program: RUNTIME_ENGINE_PATH,
        invocation: Invocation::Executable,
    },
    plist: RUNTIME_LAUNCHD_PATH,
};

/// The privileged removal helper job.
pub(crate) const REMOVAL_HELPER: Job = Job {
    identity: ServiceIdentity {
        label: crate::removal::SERVICE,
        program: REMOVAL_HELPER_PATH,
        invocation: Invocation::RemovalHelper,
    },
    plist: REMOVAL_LAUNCHD_PATH,
};

impl Job {
    /// The `system/<label>` target that bootout addresses.
    pub(crate) fn target(&self) -> String {
        format!("system/{}", self.identity.label)
    }

    pub(crate) fn plist(&self) -> &'static str {
        self.plist
    }
}

/// Queries only the job's fixed label and fails closed on uncertainty.
pub(crate) fn inspect(job: &Job) -> Result<RegistrationState, NativeError> {
    let output = command::run_command(PROGRAM, &["list", job.identity.label])?;
    status::classify(
        output.status.code(),
        &output.stdout,
        &output.stderr,
        job.identity,
    )
}

/// Registers the job from its installed plist; any output is a failure.
pub(crate) fn bootstrap(job: &Job) -> Result<(), NativeError> {
    command::run_mutation(PROGRAM, &["bootstrap", "system", job.plist])
}

/// Unregisters the job; any output is a failure.
pub(crate) fn bootout(job: &Job) -> Result<(), NativeError> {
    command::run_mutation(PROGRAM, &["bootout", &job.target()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_address_their_fixed_system_targets_and_plists() {
        assert_eq!(DRIVER.target(), "system/io.mixrelay.audio.driver");
        assert_eq!(ENGINE.target(), "system/io.mixrelay.audio.engine");
        assert_eq!(REMOVAL_HELPER.target(), "system/io.mixrelay.driver.removal");
        assert_eq!(
            [DRIVER.plist(), ENGINE.plist(), REMOVAL_HELPER.plist()],
            [LAUNCHD_PATH, RUNTIME_LAUNCHD_PATH, REMOVAL_LAUNCHD_PATH]
        );
    }
}
