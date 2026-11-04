use super::*;

const DRIVER: ServiceIdentity = super::super::DRIVER.identity;
const ENGINE: ServiceIdentity = super::super::ENGINE.identity;
const REMOVAL_HELPER: ServiceIdentity = super::super::REMOVAL_HELPER.identity;

fn loaded(bytes: &[u8], identity: ServiceIdentity) -> Result<RegistrationState, NativeError> {
    classify(Some(0), bytes, b"", identity)
}

fn driver(bytes: &[u8]) -> Result<RegistrationState, NativeError> {
    loaded(bytes, DRIVER)
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
        driver(bytes).expect("known runtime status parses"),
        RegistrationState::Loaded { pid: None }
    );
}

#[test]
fn removal_registration_accepts_only_its_fixed_invocation_and_a_valid_team() {
    let identity = REMOVAL_HELPER;
    let label = identity.label;
    let program = identity.program;
    let status = format!(
        r#"{{"Label"="{label}";"Program"="{program}";"ProgramArguments"=("{program}","removal-helper","AB12C3D4E5");"MachServices"={{"{label}"=mach-port-object;}};}}"#
    );
    assert_eq!(
        loaded(status.as_bytes(), identity).unwrap(),
        RegistrationState::Loaded { pid: None }
    );
    for invalid in [
        status.replace("AB12C3D4E5", "malformed"),
        status.replace("removal-helper", "remove"),
        status.replace(program, "/tmp/arbitrary"),
        status.replace(label, "io.mixrelay.other"),
        status.replace(",\"AB12C3D4E5\"", ""),
    ] {
        assert!(loaded(invalid.as_bytes(), identity).is_err());
    }
    assert_eq!(
        classify(Some(113), b"", b"localised diagnostic", identity).unwrap(),
        RegistrationState::Absent
    );
}

#[test]
fn engine_registration_rejects_driver_or_arbitrary_program_identity() {
    let (label, program) = (ENGINE.label, ENGINE.program);
    let status = format!(
        r#"{{"Label"="{label}";"Program"="{program}";"ProgramArguments"=("{program}");"MachServices"={{"{label}"=mach-port-object;}};}}"#
    );
    assert_eq!(
        loaded(status.as_bytes(), ENGINE).unwrap(),
        RegistrationState::Loaded { pid: None }
    );
    for invalid in [
        status.replace(program, crate::contract::DRIVER_PATH),
        status.replace(label, "io.mixrelay.audio.driver"),
        status.replace(program, "/tmp/arbitrary"),
    ] {
        assert!(loaded(invalid.as_bytes(), ENGINE).is_err());
    }
}

#[test]
fn engine_registration_accepts_launchctl_log_paths_and_checks_their_types() {
    // Shape observed from a launchd job with stdout/stderr logging enabled.
    let (label, program) = (ENGINE.label, ENGINE.program);
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
        loaded(status.as_bytes(), ENGINE).unwrap(),
        RegistrationState::Loaded { pid: None }
    );
    for path in ["engine.stdout.log", "engine.stderr.log"] {
        let quoted = format!("\"/Users/listener/Library/Logs/MixRelay/{path}\"");
        for invalid in ["123", "true", "\"relative.log\"", "\"\""] {
            let malformed = status.replace(&quoted, invalid);
            assert!(loaded(malformed.as_bytes(), ENGINE).is_err());
        }
    }
    let unknown = status.replace("\"LastExitStatus\" = 0;", "\"NewKey\" = \"unknown\";");
    assert!(loaded(unknown.as_bytes(), ENGINE).is_err());
}

#[test]
fn runtime_status_requires_exact_payload_identity_and_closed_schema() {
    let wrong_label = br#"{"Label"="com.example.other";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;};}"#;
    assert!(driver(wrong_label).is_err());
    let wrong_program = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/fixture/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;};}"#;
    assert!(driver(wrong_program).is_err());
    let wrong_args = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service","--unsafe");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;};}"#;
    assert!(driver(wrong_args).is_err());
    let unknown_key = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/x";"NewKey"=true;}"#;
    assert!(driver(unknown_key).is_err());
    assert!(driver(b"not a plist").is_err());
}

#[test]
fn absent_pid_means_registered_but_idle_and_unknown_policy_keys_are_rejected() {
    let idle = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;};}"#;
    assert_eq!(
        driver(idle).unwrap(),
        RegistrationState::Loaded { pid: None }
    );
    let pid = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;};"PID"=123;}"#;
    assert_eq!(
        driver(pid).unwrap(),
        RegistrationState::Loaded { pid: Some(123) }
    );
    let zero_pid = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;};"PID"=0;}"#;
    assert_eq!(
        driver(zero_pid).unwrap(),
        RegistrationState::Loaded { pid: None }
    );
    let policy_key = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;};"UserName"="listener";}"#;
    assert!(driver(policy_key).is_err());
}

#[test]
fn mach_port_atom_is_recognized_only_outside_strings_and_exactly_once() {
    let quoted = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"="mach-port-object";};}"#;
    assert!(driver(quoted).is_err());
    let atom_outside_service = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service";);"LimitLoadToSessionType"=mach-port-object;"MachServices"={"io.mixrelay.audio.driver"="__MIXRELAY_LAUNCHCTL_MACH_PORT_OBJECT__";};}"#;
    assert!(driver(atom_outside_service).is_err());
    let duplicate = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;"other"=mach-port-object;};}"#;
    assert!(driver(duplicate).is_err());
}

#[test]
fn successful_queries_with_stderr_or_empty_output_remain_unknown() {
    let idle = br#"{"Label"="io.mixrelay.audio.driver";"Program"="/Library/Application Support/MixRelay/bin/mixrelay-driver-service";"ProgramArguments"=("/Library/Application Support/MixRelay/bin/mixrelay-driver-service");"MachServices"={"io.mixrelay.audio.driver"=mach-port-object;};}"#;
    assert!(matches!(
        classify(Some(0), idle, b"warning", DRIVER),
        Err(NativeError::LaunchdState(_))
    ));
    assert!(matches!(
        classify(Some(0), b"", b"", DRIVER),
        Err(NativeError::LaunchdState(_))
    ));
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
            classify(Some(113), b"", diagnostic.as_bytes(), DRIVER)
                .expect("service-not-found code with empty stdout means absent"),
            RegistrationState::Absent,
            "diagnostic: {diagnostic:?}"
        );
    }
}

#[test]
fn other_failures_and_not_found_with_stdout_remain_errors() {
    let diagnostic = b"Could not find service \"io.mixrelay.audio.driver\" in domain for system\n";
    for exit_code in [Some(1), Some(112), Some(114), Some(127), None] {
        assert!(matches!(
            classify(exit_code, b"", diagnostic, DRIVER),
            Err(NativeError::CommandResult { .. })
        ));
    }
    for stderr in [diagnostic.as_slice(), b""] {
        for stdout in ["unexpected payload", "\n"] {
            assert!(matches!(
                classify(Some(113), stdout.as_bytes(), stderr, DRIVER),
                Err(NativeError::CommandResult { .. })
            ));
        }
    }
}
