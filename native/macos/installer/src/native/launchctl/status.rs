//! Classifies `launchctl list <label>` results into a closed registration state.
//!
//! `launchctl list` does not expose the job's configured account, process type,
//! restart policy, environment, or a reliably selected bootstrap domain. This
//! observation is not treated as proof of those fields: lifecycle callers
//! separately verify the signed on-disk launchd policy.

use std::io;

use plist::{Dictionary, Value};

use super::atoms::{
    FALSE_MARKER, LaunchctlAtoms, MACH_PORT_OBJECT_MARKER, TRUE_MARKER, quote_launchctl_atoms,
};
use crate::native::{
    NativeError,
    command::{exit_code_string, output_detail},
};

const MAX_STATUS_BYTES: usize = 64 * 1024;
const KNOWN_STATUS_KEYS: &[&str] = &[
    "Label",
    "Program",
    "ProgramArguments",
    "PID",
    "OnDemand",
    "LastExitStatus",
    "EnableTransactions",
    "LimitLoadToSessionType",
    "MachServices",
    "StandardOutPath",
    "StandardErrorPath",
];

// For the fixed-label list query, launchctl error 113 means the specified service
// was not found. Stderr is a diagnostic; its wording and domain text are not a contract.
const ABSENT_EXIT_CODE: i32 = 113;

/// Runtime registration state observed for one exact job label.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RegistrationState {
    /// The exact label was reported absent by the command's current launchd context.
    Absent,
    /// The exact service target is registered; `pid` is present only while running.
    Loaded { pid: Option<u32> },
}

/// The arguments a registered job may carry after its program path.
#[derive(Clone, Copy, Debug)]
pub(super) enum Invocation {
    /// Only the program itself.
    Executable,
    /// `removal-helper <Team ID>`.
    RemovalHelper,
}

/// The label, program and invocation a registered job must report.
#[derive(Clone, Copy, Debug)]
pub(super) struct ServiceIdentity {
    pub(super) label: &'static str,
    pub(super) program: &'static str,
    pub(super) invocation: Invocation,
}

/// Classifies a list query from its exit code and output, failing closed on anything unknown.
pub(super) fn classify(
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
    identity: ServiceIdentity,
) -> Result<RegistrationState, NativeError> {
    if exit_code == Some(ABSENT_EXIT_CODE) && stdout.is_empty() {
        return Ok(RegistrationState::Absent);
    }
    if exit_code != Some(0) {
        return Err(NativeError::CommandResult {
            program: super::PROGRAM,
            status: exit_code_string(exit_code),
            detail: output_detail(stdout, stderr),
        });
    }
    if !stderr.is_empty() {
        return Err(NativeError::LaunchdState(
            "successful status query unexpectedly wrote to stderr",
        ));
    }
    parse_loaded(stdout, identity)
}

fn parse_loaded(bytes: &[u8], identity: ServiceIdentity) -> Result<RegistrationState, NativeError> {
    if bytes.is_empty() || bytes.len() > MAX_STATUS_BYTES {
        return Err(NativeError::LaunchdState(
            "successful exact-label query returned empty or oversized output",
        ));
    }
    // An active Mach service is rendered as the bare atom `mach-port-object`; it is
    // accepted only at the fixed MachServices entry. Policy fields omitted by this
    // status format remain unverified here; NativeServiceLifecycle validates the plist.
    let (normalized, atoms) = quote_launchctl_atoms(bytes)?;
    let value = Value::from_reader_ascii(io::Cursor::new(normalized))
        .map_err(|_| NativeError::LaunchdState("exact-label result is not an ASCII plist"))?;
    let Some(dictionary) = value.as_dictionary() else {
        return Err(NativeError::LaunchdState(
            "exact-label result is not a dictionary",
        ));
    };
    validate_schema(dictionary, &value, atoms)?;
    validate_identity(dictionary, identity)?;
    validate_runtime_fields(dictionary)?;
    Ok(RegistrationState::Loaded {
        pid: parse_pid(dictionary)?,
    })
}

/// Every quoted marker must come from a bare atom, and only known keys may appear.
fn validate_schema(
    dictionary: &Dictionary,
    value: &Value,
    atoms: LaunchctlAtoms,
) -> Result<(), NativeError> {
    if atoms.mach_port != 1 || count_marker(value, MACH_PORT_OBJECT_MARKER) != 1 {
        return Err(NativeError::LaunchdState(
            "launchctl MachServices marker appears outside its one fixed service entry",
        ));
    }
    if count_marker(value, TRUE_MARKER) != atoms.true_values
        || count_marker(value, FALSE_MARKER) != atoms.false_values
    {
        return Err(NativeError::LaunchdState(
            "launchctl boolean markers appear outside their observed runtime fields",
        ));
    }
    if dictionary
        .keys()
        .any(|key| !KNOWN_STATUS_KEYS.contains(&key.as_str()))
    {
        return Err(NativeError::LaunchdState(
            "exact-label result contains an unknown schema key",
        ));
    }
    Ok(())
}

fn validate_identity(
    dictionary: &Dictionary,
    identity: ServiceIdentity,
) -> Result<(), NativeError> {
    if dictionary
        .get("Label")
        .and_then(Value::as_string)
        .is_none_or(|label| label != identity.label)
    {
        return Err(NativeError::LaunchdState(
            "exact-label result is missing the fixed service label",
        ));
    }
    let program =
        dictionary
            .get("Program")
            .and_then(Value::as_string)
            .ok_or(NativeError::LaunchdState(
                "exact-label result is missing or has malformed Program",
            ))?;
    let arguments = dictionary
        .get("ProgramArguments")
        .and_then(Value::as_array)
        .ok_or(NativeError::LaunchdState(
            "exact-label result is missing or has malformed ProgramArguments",
        ))?;
    if program != identity.program {
        return Err(NativeError::LaunchdState(
            "registered executable differs from the fixed engine path",
        ));
    }
    if !arguments_match(arguments, identity) {
        return Err(NativeError::LaunchdState(
            "registered arguments differ from the fixed engine invocation",
        ));
    }
    validate_mach_services(dictionary, identity.label)
}

fn arguments_match(arguments: &[Value], identity: ServiceIdentity) -> bool {
    let tail_matches = match identity.invocation {
        Invocation::Executable => arguments.len() == 1,
        Invocation::RemovalHelper => {
            arguments.len() == 3
                && arguments.get(1).and_then(Value::as_string) == Some("removal-helper")
                && arguments
                    .get(2)
                    .and_then(Value::as_string)
                    .is_some_and(|team| crate::SigningTeam::new(team.to_owned()).is_ok())
        }
    };
    tail_matches && arguments.first().and_then(Value::as_string) == Some(identity.program)
}

fn validate_mach_services(dictionary: &Dictionary, label: &str) -> Result<(), NativeError> {
    let Some(services) = dictionary
        .get("MachServices")
        .and_then(Value::as_dictionary)
    else {
        return Err(NativeError::LaunchdState(
            "registered MachServices is missing or not a dictionary",
        ));
    };
    if services.len() != 1
        || services.get(label).and_then(Value::as_string) != Some(MACH_PORT_OBJECT_MARKER)
    {
        return Err(NativeError::LaunchdState(
            "runtime MachServices differs from the fixed product service",
        ));
    }
    Ok(())
}

fn validate_runtime_fields(dictionary: &Dictionary) -> Result<(), NativeError> {
    if let Some(value) = dictionary.get("PID")
        && value.as_unsigned_integer().is_none()
    {
        return Err(NativeError::LaunchdState(
            "registered PID has an unexpected type",
        ));
    }
    validate_boolean(dictionary, "OnDemand")?;
    if let Some(value) = dictionary.get("LastExitStatus")
        && value.as_signed_integer().is_none()
    {
        return Err(NativeError::LaunchdState(
            "LastExitStatus has an unexpected type",
        ));
    }
    validate_boolean(dictionary, "EnableTransactions")?;
    if let Some(value) = dictionary.get("LimitLoadToSessionType")
        && value.as_string().is_none()
    {
        return Err(NativeError::LaunchdState(
            "LimitLoadToSessionType has an unexpected type",
        ));
    }
    // launchctl reports these paths for the engine's configured log files.
    // They are status metadata; the signed, account-bound plist is checked separately.
    for key in ["StandardOutPath", "StandardErrorPath"] {
        if let Some(value) = dictionary.get(key)
            && value.as_string().is_none_or(|path| !path.starts_with('/'))
        {
            return Err(NativeError::LaunchdState(
                "runtime log path is not an absolute string path",
            ));
        }
    }
    Ok(())
}

fn validate_boolean(dictionary: &Dictionary, key: &str) -> Result<(), NativeError> {
    if let Some(value) = dictionary.get(key)
        && !matches!(value.as_string(), Some(TRUE_MARKER | FALSE_MARKER))
    {
        return Err(NativeError::LaunchdState(
            "launchctl boolean field has an unexpected type",
        ));
    }
    Ok(())
}

/// A reported PID of zero counts as not running.
fn parse_pid(dictionary: &Dictionary) -> Result<Option<u32>, NativeError> {
    dictionary
        .get("PID")
        .map(|value| {
            let pid = value
                .as_unsigned_integer()
                .ok_or(NativeError::LaunchdState(
                    "registered PID has an unexpected type",
                ))?;
            u32::try_from(pid).map_err(|_| {
                NativeError::LaunchdState("registered PID exceeds the supported range")
            })
        })
        .transpose()
        .map(|pid| pid.filter(|pid| *pid != 0))
}

fn count_marker(value: &Value, marker: &str) -> usize {
    match value {
        Value::String(value) => usize::from(value == marker),
        Value::Array(values) => values.iter().map(|value| count_marker(value, marker)).sum(),
        Value::Dictionary(values) => values
            .iter()
            .map(|(key, value)| usize::from(key == marker) + count_marker(value, marker))
            .sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests;
