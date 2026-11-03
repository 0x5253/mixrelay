//! Bounded launchctl integration and fixed Core Audio process reload.

use std::{
    io::{self, Read},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use plist::Value;

use crate::contract::DRIVER_PATH;

use super::NativeError;

const PROGRAM: &str = "/bin/launchctl";
const COREAUDIO_RELOAD_PROGRAM: &str = "/usr/bin/killall";
const SERVICE_LABEL: &str = "io.mixrelay.audio.driver";
const SERVICE_TARGET: &str = "system/io.mixrelay.audio.driver";
const MAX_CHILD_OUTPUT: usize = 64 * 1024;
const CHILD_TIMEOUT: Duration = Duration::from_secs(10);
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

/// Runtime registration state observed for the exact product label.
///
/// `launchctl list` does not expose the job's configured account, process type,
/// restart policy, environment, or a reliably selected bootstrap domain. This
/// observation is not treated as proof of those fields: lifecycle callers
/// separately verify the signed on-disk launchd policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RegistrationState {
    /// The exact label was reported absent by the command's current launchd context.
    Absent,
    /// The exact service target is registered; `pid` is present only while running.
    Loaded { pid: Option<u32> },
}

#[derive(Clone, Copy)]
enum Invocation {
    Executable,
    RemovalHelper,
}
#[derive(Clone, Copy)]
struct ServiceIdentity<'a> {
    label: &'a str,
    program: &'a str,
    invocation: Invocation,
}

/// Queries only the product's fixed launchd label and fails closed on uncertainty.
pub(super) fn inspect_registration() -> Result<RegistrationState, NativeError> {
    let output = run_command(PROGRAM, &["list", SERVICE_LABEL])?;
    classify_status_output(output.status.code(), &output.stdout, &output.stderr)
}

fn classify_status_output(
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<RegistrationState, NativeError> {
    classify_status_output_for(
        exit_code,
        stdout,
        stderr,
        ServiceIdentity {
            label: SERVICE_LABEL,
            program: DRIVER_PATH,
            invocation: Invocation::Executable,
        },
    )
}

pub(super) fn inspect_engine_registration() -> Result<RegistrationState, NativeError> {
    let output = run_command(PROGRAM, &["list", "io.mixrelay.audio.engine"])?;
    classify_status_output_for(
        output.status.code(),
        &output.stdout,
        &output.stderr,
        ServiceIdentity {
            label: "io.mixrelay.audio.engine",
            program: crate::RUNTIME_ENGINE_PATH,
            invocation: Invocation::Executable,
        },
    )
}

pub(crate) fn removal_helper_is_registered() -> Result<bool, NativeError> {
    let output = run_command(PROGRAM, &["list", crate::removal::SERVICE])?;
    let state = classify_status_output_for(
        output.status.code(),
        &output.stdout,
        &output.stderr,
        ServiceIdentity {
            label: crate::removal::SERVICE,
            program: crate::REMOVAL_HELPER_PATH,
            invocation: Invocation::RemovalHelper,
        },
    )?;
    Ok(matches!(state, RegistrationState::Loaded { .. }))
}

pub(crate) fn bootstrap_removal_helper() -> Result<(), NativeError> {
    run_mutation(
        PROGRAM,
        &["bootstrap", "system", crate::REMOVAL_LAUNCHD_PATH],
    )
}

pub(crate) fn bootout_removal_helper() -> Result<(), NativeError> {
    run_mutation(PROGRAM, &["bootout", "system/io.mixrelay.driver.removal"])
}

fn classify_status_output_for(
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
    identity: ServiceIdentity<'_>,
) -> Result<RegistrationState, NativeError> {
    if exit_code == Some(ABSENT_EXIT_CODE) && stdout.is_empty() {
        return Ok(RegistrationState::Absent);
    }
    if exit_code != Some(0) {
        return Err(NativeError::CommandResult {
            program: PROGRAM,
            status: exit_code.map_or_else(
                || "terminated by signal".to_owned(),
                |code| code.to_string(),
            ),
            detail: format!("stdout={}; stderr={}", escaped(stdout), escaped(stderr)),
        });
    }
    if !stderr.is_empty() {
        return Err(NativeError::LaunchdState(
            "successful status query unexpectedly wrote to stderr",
        ));
    }
    parse_status_identity(stdout, identity)
}

pub(super) fn bootstrap() -> Result<(), NativeError> {
    run_mutation(PROGRAM, &["bootstrap", "system", crate::LAUNCHD_PATH])
}

pub(super) fn bootout() -> Result<(), NativeError> {
    run_mutation(PROGRAM, &["bootout", SERVICE_TARGET])
}

/// Force-terminates only the OS-owned Core Audio server so launchd reloads the HAL.
/// The lifecycle requires explicit audio-interruption approval before this call.
pub(super) fn reload_coreaudio() -> Result<(), NativeError> {
    tracing::info!(strategy = ?super::hal_reload::ReloadStrategy::FullCoreAudio, "Reloading CoreAudio; active system audio will be interrupted");
    run_mutation(
        COREAUDIO_RELOAD_PROGRAM,
        &["-KILL", "-u", "_coreaudiod", "coreaudiod"],
    )
}

fn run_mutation(program: &'static str, arguments: &[&str]) -> Result<(), NativeError> {
    let output = run_command(program, arguments)?;
    if output.status.success() && output.stdout.is_empty() && output.stderr.is_empty() {
        Ok(())
    } else {
        Err(command_result(
            program,
            output.status,
            &output.stdout,
            &output.stderr,
        ))
    }
}

pub(super) fn run_command(
    program: &'static str,
    arguments: &[&str],
) -> Result<ChildOutput, NativeError> {
    let child = Command::new(program)
        .env_clear()
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| NativeError::CommandIo { program, source })?;
    let mut owner = ChildOwner(child);
    let stdout = owner
        .0
        .stdout
        .take()
        .ok_or_else(|| NativeError::CommandIo {
            program,
            source: io::Error::other("stdout pipe was not created"),
        })?;
    let stderr = owner
        .0
        .stderr
        .take()
        .ok_or_else(|| NativeError::CommandIo {
            program,
            source: io::Error::other("stderr pipe was not created"),
        })?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout, MAX_CHILD_OUTPUT));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, MAX_CHILD_OUTPUT));
    let status_result = wait_bounded(program, &mut owner.0);
    let stdout_result = stdout_reader
        .join()
        .map_err(|_| NativeError::CommandResult {
            program,
            status: status_result
                .as_ref()
                .map_or_else(|_| "unknown".to_owned(), |status| status_string(*status)),
            detail: "stdout collection thread panicked".to_owned(),
        })?
        .map_err(|source| NativeError::CommandIo { program, source });
    let stderr_result = stderr_reader
        .join()
        .map_err(|_| NativeError::CommandResult {
            program,
            status: status_result
                .as_ref()
                .map_or_else(|_| "unknown".to_owned(), |status| status_string(*status)),
            detail: "stderr collection thread panicked".to_owned(),
        })?
        .map_err(|source| NativeError::CommandIo { program, source });
    let status = status_result?;
    let stdout = stdout_result?;
    let stderr = stderr_result?;
    if stdout.truncated || stderr.truncated {
        return Err(NativeError::CommandResult {
            program,
            status: status_string(status),
            detail: "command exceeded the bounded output budget".to_owned(),
        });
    }
    Ok(ChildOutput {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    })
}

fn wait_bounded(program: &'static str, child: &mut Child) -> Result<ExitStatus, NativeError> {
    let deadline = Instant::now() + CHILD_TIMEOUT;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|source| NativeError::CommandIo { program, source })?
        {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            child
                .kill()
                .map_err(|source| NativeError::CommandIo { program, source })?;
            child
                .wait()
                .map_err(|source| NativeError::CommandIo { program, source })?;
            return Err(NativeError::CommandResult {
                program,
                status: "timeout".to_owned(),
                detail: "command exceeded the ten-second deadline and was reaped".to_owned(),
            });
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn read_bounded(mut reader: impl Read, limit: usize) -> io::Result<BoundedBytes> {
    let mut bytes = Vec::with_capacity(limit.min(4096));
    let mut truncated = false;
    let mut chunk = [0; 4096];
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        let available = limit.saturating_sub(bytes.len());
        let kept = count.min(available);
        bytes.extend_from_slice(&chunk[..kept]);
        truncated |= kept != count;
    }
    Ok(BoundedBytes { bytes, truncated })
}

#[cfg(test)]
fn parse_status(bytes: &[u8]) -> Result<RegistrationState, NativeError> {
    parse_status_for(bytes, SERVICE_LABEL, DRIVER_PATH)
}

#[cfg(test)]
fn parse_status_for(
    bytes: &[u8],
    label: &str,
    program: &str,
) -> Result<RegistrationState, NativeError> {
    if bytes.is_empty() || bytes.len() > MAX_CHILD_OUTPUT {
        return Err(NativeError::LaunchdState(
            "successful exact-label query returned empty or oversized output",
        ));
    }
    parse_status_identity(
        bytes,
        ServiceIdentity {
            label,
            program,
            invocation: Invocation::Executable,
        },
    )
}

fn parse_status_identity(
    bytes: &[u8],
    identity: ServiceIdentity<'_>,
) -> Result<RegistrationState, NativeError> {
    if bytes.is_empty() || bytes.len() > MAX_CHILD_OUTPUT {
        return Err(NativeError::LaunchdState(
            "successful exact-label query returned empty or oversized output",
        ));
    }
    // `launchctl list <label>` emits runtime state in an OpenStep dictionary,
    // not the source plist. In particular, an active Mach service is rendered
    // as the launchctl-specific bare atom `mach-port-object`, which plist
    // readers do not recognize. Quote only that exact token outside
    // strings/comments, then accept it only at the fixed MachServices entry.
    // Policy fields omitted by this status format remain unverified here; their
    // source plist is validated independently by NativeServiceLifecycle.
    let (normalized, atoms) = quote_launchctl_atoms(bytes)?;
    let value = Value::from_reader_ascii(io::Cursor::new(normalized))
        .map_err(|_| NativeError::LaunchdState("exact-label result is not an ASCII plist"))?;
    let Some(dictionary) = value.as_dictionary() else {
        return Err(NativeError::LaunchdState(
            "exact-label result is not a dictionary",
        ));
    };
    validate_status_schema(dictionary, &value, atoms)?;
    validate_status_identity(dictionary, atoms.mach_port, identity)?;
    validate_runtime_fields(dictionary)?;
    Ok(RegistrationState::Loaded {
        pid: parse_pid(dictionary)?,
    })
}

fn validate_status_schema(
    dictionary: &plist::Dictionary,
    value: &Value,
    atoms: LaunchctlAtoms,
) -> Result<(), NativeError> {
    if atoms.mach_port != 1 || count_status_marker(value, MACH_PORT_OBJECT_MARKER) != 1 {
        return Err(NativeError::LaunchdState(
            "launchctl MachServices marker appears outside its one fixed service entry",
        ));
    }
    if count_status_marker(value, TRUE_MARKER) != atoms.true_values
        || count_status_marker(value, FALSE_MARKER) != atoms.false_values
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

fn validate_status_identity(
    dictionary: &plist::Dictionary,
    mach_port_atoms: usize,
    identity: ServiceIdentity<'_>,
) -> Result<(), NativeError> {
    let expected_label = identity.label;
    let expected_program = identity.program;
    if dictionary
        .get("Label")
        .and_then(Value::as_string)
        .is_none_or(|label| label != expected_label)
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
    if program != expected_program {
        return Err(NativeError::LaunchdState(
            "registered executable differs from the fixed engine path",
        ));
    }
    let valid_arguments = match identity.invocation {
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
    if !valid_arguments || arguments.first().and_then(Value::as_string) != Some(expected_program) {
        return Err(NativeError::LaunchdState(
            "registered arguments differ from the fixed engine invocation",
        ));
    }
    if mach_port_atoms != 1 {
        return Err(NativeError::LaunchdState(
            "exact-label result must contain exactly one launchctl MachServices object",
        ));
    }
    validate_runtime_mach_services(dictionary, expected_label)?;
    Ok(())
}

fn validate_runtime_fields(dictionary: &plist::Dictionary) -> Result<(), NativeError> {
    if let Some(value) = dictionary.get("PID")
        && value.as_unsigned_integer().is_none()
    {
        return Err(NativeError::LaunchdState(
            "registered PID has an unexpected type",
        ));
    }
    validate_runtime_boolean(dictionary, "OnDemand")?;
    if let Some(value) = dictionary.get("LastExitStatus")
        && value.as_signed_integer().is_none()
    {
        return Err(NativeError::LaunchdState(
            "LastExitStatus has an unexpected type",
        ));
    }
    validate_runtime_boolean(dictionary, "EnableTransactions")?;
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

fn validate_runtime_boolean(dictionary: &plist::Dictionary, key: &str) -> Result<(), NativeError> {
    if let Some(value) = dictionary.get(key)
        && !matches!(value.as_string(), Some(TRUE_MARKER | FALSE_MARKER))
    {
        return Err(NativeError::LaunchdState(
            "launchctl boolean field has an unexpected type",
        ));
    }
    Ok(())
}

fn parse_pid(dictionary: &plist::Dictionary) -> Result<Option<u32>, NativeError> {
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

fn validate_runtime_mach_services(
    dictionary: &plist::Dictionary,
    label: &str,
) -> Result<(), NativeError> {
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

fn count_status_marker(value: &Value, marker: &str) -> usize {
    match value {
        Value::String(value) => usize::from(value == marker),
        Value::Array(values) => values
            .iter()
            .map(|value| count_status_marker(value, marker))
            .sum(),
        Value::Dictionary(values) => values
            .iter()
            .map(|(key, value)| {
                let key_is_marker = usize::from(key == marker);
                key_is_marker + count_status_marker(value, marker)
            })
            .sum(),
        _ => 0,
    }
}

const MACH_PORT_OBJECT_MARKER: &str = "__MIXRELAY_LAUNCHCTL_MACH_PORT_OBJECT__";
const TRUE_MARKER: &str = "__MIXRELAY_LAUNCHCTL_TRUE__";
const FALSE_MARKER: &str = "__MIXRELAY_LAUNCHCTL_FALSE__";

/// Quotes the launchctl status atoms that are not `OpenStep` plist values.
fn quote_launchctl_atoms(bytes: &[u8]) -> Result<(Vec<u8>, LaunchctlAtoms), NativeError> {
    let mut normalized = Vec::with_capacity(bytes.len() + 32);
    let mut index = 0;
    let mut replacements = LaunchctlAtoms::default();
    let mut quoted = false;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut dictionary_depth = 0usize;
    let mut array_dictionary_depths = Vec::new();
    while index < bytes.len() {
        let current = bytes[index];
        if quoted {
            normalized.push(current);
            index += 1;
            if escaped {
                escaped = false;
            } else if current == b'\\' {
                escaped = true;
            } else if current == b'"' {
                quoted = false;
            }
            continue;
        }
        if line_comment {
            normalized.push(current);
            index += 1;
            line_comment = current != b'\n';
            continue;
        }
        if block_comment {
            normalized.push(current);
            index += 1;
            if current == b'*' && bytes.get(index) == Some(&b'/') {
                normalized.push(b'/');
                index += 1;
                block_comment = false;
            }
            continue;
        }
        if current == b'"' {
            quoted = true;
            normalized.push(current);
            index += 1;
            continue;
        }
        if current == b'/' && bytes.get(index + 1) == Some(&b'/') {
            line_comment = true;
            normalized.extend_from_slice(b"//");
            index += 2;
            continue;
        }
        if current == b'/' && bytes.get(index + 1) == Some(&b'*') {
            block_comment = true;
            normalized.extend_from_slice(b"/*");
            index += 2;
            continue;
        }
        match current {
            b'{' => dictionary_depth = dictionary_depth.saturating_add(1),
            b'}' => dictionary_depth = dictionary_depth.saturating_sub(1),
            b'(' => array_dictionary_depths.push(dictionary_depth),
            b')' => {
                array_dictionary_depths.pop();
            }
            b';' if array_dictionary_depths.last() == Some(&dictionary_depth) => {
                let next = bytes[index + 1..]
                    .iter()
                    .copied()
                    .find(|byte| !byte.is_ascii_whitespace());
                if next != Some(b')') {
                    normalized.push(b',');
                }
                index += 1;
                continue;
            }
            _ => {}
        }
        let replacement = [
            (b"mach-port-object".as_slice(), MACH_PORT_OBJECT_MARKER),
            (b"true".as_slice(), TRUE_MARKER),
            (b"false".as_slice(), FALSE_MARKER),
        ]
        .iter()
        .find_map(|(atom, marker)| {
            let end = index + atom.len();
            (bytes.get(index..end) == Some(*atom)
                && !is_atom_byte(index.checked_sub(1).and_then(|before| bytes.get(before)))
                && !is_atom_byte(bytes.get(end)))
            .then_some((end, *marker))
        });
        if let Some((end, marker)) = replacement {
            match marker {
                MACH_PORT_OBJECT_MARKER => replacements.mach_port += 1,
                TRUE_MARKER => replacements.true_values += 1,
                FALSE_MARKER => replacements.false_values += 1,
                _ => unreachable!("replacement markers are closed constants"),
            }
            normalized.push(b'"');
            normalized.extend_from_slice(marker.as_bytes());
            normalized.push(b'"');
            index = end;
            continue;
        }
        normalized.push(current);
        index += 1;
    }
    if quoted || block_comment {
        return Err(NativeError::LaunchdState(
            "exact-label result has unterminated string or comment",
        ));
    }
    Ok((normalized, replacements))
}

#[derive(Clone, Copy, Debug, Default)]
struct LaunchctlAtoms {
    mach_port: usize,
    true_values: usize,
    false_values: usize,
}

fn is_atom_byte(byte: Option<&u8>) -> bool {
    byte.is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
}

fn command_result(
    program: &'static str,
    status: ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> NativeError {
    let detail = format!("stdout={}; stderr={}", escaped(stdout), escaped(stderr));
    NativeError::CommandResult {
        program,
        status: status_string(status),
        detail,
    }
}

fn status_string(status: ExitStatus) -> String {
    status.code().map_or_else(
        || "terminated by signal".to_owned(),
        |code| code.to_string(),
    )
}

fn escaped(bytes: &[u8]) -> String {
    let mut result = String::new();
    for character in String::from_utf8_lossy(bytes).chars() {
        if result.len() >= 2048 {
            result.push('…');
            break;
        }
        result.extend(character.escape_default());
    }
    result
}

pub(super) struct ChildOutput {
    pub(super) status: ExitStatus,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
}

struct BoundedBytes {
    bytes: Vec<u8>,
    truncated: bool,
}

struct ChildOwner(Child);

impl Drop for ChildOwner {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_launchctl_command_failures_report_the_actual_program() {
        let error = run_mutation("/usr/bin/false", &[]).expect_err("false must fail");
        assert!(matches!(
            error,
            NativeError::CommandResult { program: "/usr/bin/false", status, .. }
                if status == "1"
        ));

        let error = run_command("/nonexistent-mixrelay-command", &[])
            .err()
            .expect("missing command must fail to spawn");
        assert!(matches!(
            error,
            NativeError::CommandIo {
                program: "/nonexistent-mixrelay-command",
                ..
            }
        ));
    }

    #[test]
    fn bounded_command_collection_preserves_stdout_stderr_and_exit_status() {
        let output = run_command(
            "/bin/sh",
            &["-c", "printf 'fixture stdout'; printf 'fixture stderr' >&2"],
        )
        .expect("fixture command succeeds");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"fixture stdout");
        assert_eq!(output.stderr, b"fixture stderr");
    }

    #[test]
    fn bounded_collection_drains_output_after_its_budget_is_exhausted() {
        let mut reader = io::Cursor::new(b"12345");
        let output = read_bounded(&mut reader, 3).expect("fixture bytes are readable");
        assert_eq!(output.bytes, b"123");
        assert!(output.truncated);
        assert_eq!(reader.position(), 5);
    }

    #[test]
    fn launchctl_runtime_status_shape_is_parsed_without_inventing_disk_policy_fields() {
        let bytes = br#"{
            "EnableTransactions" = true;
            "LimitLoadToSessionType" = "Background";
            "MachServices" = { "io.mixrelay.audio.driver" = mach-port-object; };
            "Label" = "io.mixrelay.audio.driver";
            "OnDemand" = true;
            "LastExitStatus" = 0;
            "Program" = "/Library/Application Support/MixRelay/bin/mixrelay-driver-service";
            "ProgramArguments" = ("/Library/Application Support/MixRelay/bin/mixrelay-driver-service";);
        };"#;
        assert_eq!(
            parse_status(bytes).expect("known runtime status parses"),
            RegistrationState::Loaded { pid: None }
        );
    }

    #[test]
    fn removal_registration_accepts_only_its_fixed_invocation_and_a_valid_team() {
        let identity = ServiceIdentity {
            label: crate::removal::SERVICE,
            program: crate::REMOVAL_HELPER_PATH,
            invocation: Invocation::RemovalHelper,
        };
        let label = identity.label;
        let program = identity.program;
        let status = format!(
            r#"{{"Label"="{label}";"Program"="{program}";"ProgramArguments"=("{program}","removal-helper","AB12C3D4E5");"MachServices"={{"{label}"=mach-port-object;}};}}"#
        );
        assert_eq!(
            classify_status_output_for(Some(0), status.as_bytes(), b"", identity).unwrap(),
            RegistrationState::Loaded { pid: None }
        );
        for invalid in [
            status.replace("AB12C3D4E5", "malformed"),
            status.replace("removal-helper", "remove"),
            status.replace(program, "/tmp/arbitrary"),
            status.replace(label, "io.mixrelay.other"),
            status.replace(",\"AB12C3D4E5\"", ""),
        ] {
            assert!(
                classify_status_output_for(Some(0), invalid.as_bytes(), b"", identity).is_err()
            );
        }
        assert_eq!(
            classify_status_output_for(Some(113), b"", b"localised diagnostic", identity).unwrap(),
            RegistrationState::Absent
        );
    }

    #[test]
    fn engine_registration_rejects_driver_or_arbitrary_program_identity() {
        let label = "io.mixrelay.audio.engine";
        let program = crate::RUNTIME_ENGINE_PATH;
        let status = format!(
            r#"{{"Label"="{label}";"Program"="{program}";"ProgramArguments"=("{program}");"MachServices"={{"{label}"=mach-port-object;}};}}"#
        );
        assert_eq!(
            parse_status_for(status.as_bytes(), label, program).unwrap(),
            RegistrationState::Loaded { pid: None }
        );
        for invalid in [
            status.replace(program, crate::contract::DRIVER_PATH),
            status.replace(label, "io.mixrelay.audio.driver"),
            status.replace(program, "/tmp/arbitrary"),
        ] {
            assert!(parse_status_for(invalid.as_bytes(), label, program).is_err());
        }
    }

    #[test]
    fn engine_registration_accepts_launchctl_log_paths_and_checks_their_types() {
        // Shape observed from a launchd job with stdout/stderr logging enabled.
        let label = "io.mixrelay.audio.engine";
        let program = crate::RUNTIME_ENGINE_PATH;
        let status = format!(
            r#"{{
                "StandardOutPath" = "/Users/listener/Library/Logs/MixRelay/engine.stdout.log";
                "LimitLoadToSessionType" = "Background";
                "StandardErrorPath" = "/Users/listener/Library/Logs/MixRelay/engine.stderr.log";
                "MachServices" = {{ "{label}" = mach-port-object; }};
                "Label" = "{label}";
                "OnDemand" = true;
                "LastExitStatus" = 0;
                "Program" = "{program}";
                "ProgramArguments" = ("{program}";);
            }};"#
        );
        assert_eq!(
            parse_status_for(status.as_bytes(), label, program).unwrap(),
            RegistrationState::Loaded { pid: None }
        );
        for path in ["engine.stdout.log", "engine.stderr.log"] {
            let quoted = format!("\"/Users/listener/Library/Logs/MixRelay/{path}\"");
            for invalid in ["123", "true", "\"relative.log\"", "\"\""] {
                let malformed = status.replace(&quoted, invalid);
                assert!(parse_status_for(malformed.as_bytes(), label, program).is_err());
            }
        }
        let unknown = status.replace("\"LastExitStatus\" = 0;", "\"NewKey\" = \"unknown\";");
        assert!(parse_status_for(unknown.as_bytes(), label, program).is_err());
    }

    #[test]
    fn runtime_status_requires_exact_payload_identity_and_closed_schema() {
        let wrong_label = br#"{"Label"="com.example.other";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;};}"#;
        assert!(parse_status(wrong_label).is_err());
        let wrong_program = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/fixture/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;};}"#;
        assert!(parse_status(wrong_program).is_err());
        let wrong_args = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service","--unsafe");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;};}"#;
        assert!(parse_status(wrong_args).is_err());
        let unknown_key = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/x";"NewKey"=true;}"#;
        assert!(parse_status(unknown_key).is_err());
        assert!(parse_status(b"not a plist").is_err());
    }

    #[test]
    fn absent_pid_means_registered_but_idle_and_unknown_policy_keys_are_rejected() {
        let idle = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;};}"#;
        assert_eq!(
            parse_status(idle).unwrap(),
            RegistrationState::Loaded { pid: None }
        );
        let pid = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;};"PID"=123;}"#;
        assert_eq!(
            parse_status(pid).unwrap(),
            RegistrationState::Loaded { pid: Some(123) }
        );
        let policy_key = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;};"UserName"="listener";}"#;
        assert!(parse_status(policy_key).is_err());
    }

    #[test]
    fn mach_port_atom_is_recognized_only_outside_strings_and_exactly_once() {
        let quoted = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"="mach-port-object";};}"#;
        assert!(parse_status(quoted).is_err());
        let atom_outside_service = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service";);"LimitLoadToSessionType"=mach-port-object;"MachServices"={"io.mixrelay.audio.driver"="__MIXRELAY_LAUNCHCTL_MACH_PORT_OBJECT__";};}"#;
        assert!(parse_status(atom_outside_service).is_err());
        let duplicate = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;"other"=mach-port-object;};}"#;
        assert!(parse_status(duplicate).is_err());
    }

    #[test]
    fn service_not_found_does_not_depend_on_diagnostic_wording_or_domain() {
        assert_eq!(ABSENT_EXIT_CODE, 113);
        for diagnostic in [
            "Could not find service \"io.mixrelay.audio.driver\" in domain for system\n",
            "Could not find service \"io.mixrelay.audio.driver\" in domain for port\n",
            "지정한 서비스를 찾을 수 없습니다\n",
            "Could not find service: changed diagnostic\n",
            "",
        ] {
            assert_eq!(
                classify_status_output(Some(113), b"", diagnostic.as_bytes())
                    .expect("service-not-found code with empty stdout means absent"),
                RegistrationState::Absent,
                "diagnostic: {diagnostic:?}"
            );
        }
    }

    #[test]
    fn other_failures_and_not_found_with_stdout_remain_errors() {
        let diagnostic =
            b"Could not find service \"io.mixrelay.audio.driver\" in domain for system\n";
        for exit_code in [Some(1), Some(112), Some(114), Some(127), None] {
            assert!(matches!(
                classify_status_output(exit_code, b"", diagnostic),
                Err(NativeError::CommandResult { .. })
            ));
        }
        for stderr in [diagnostic.as_slice(), b""] {
            for stdout in ["unexpected payload", "\n"] {
                assert!(matches!(
                    classify_status_output(Some(113), stdout.as_bytes(), stderr),
                    Err(NativeError::CommandResult { .. })
                ));
            }
        }
    }
}
