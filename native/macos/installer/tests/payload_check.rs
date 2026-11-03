#![cfg(unix)]

use std::process::Command;

#[test]
fn cli_rejects_missing_and_ambiguous_inputs_before_touching_installed_files() {
    let binary = env!("CARGO_BIN_EXE_mixrelay-macos-payload-check");
    let output = Command::new(binary).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains("usage:"));

    let output = Command::new(binary)
        .args([
            "relative",
            "ABCD123456",
            "listener",
            "staff",
            "/Users/listener",
            "/not-read/mixrelay-macos-installer",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("payload root must be absolute")
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn cli_rejects_relative_installer_before_reading_payload_files() {
    let output = Command::new(env!("CARGO_BIN_EXE_mixrelay-macos-payload-check"))
        .args([
            "/not-read/payload",
            "ABCD123456",
            "listener",
            "staff",
            "/Users/listener",
            "relative-installer",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("installer executable must be absolute")
    );
    assert!(output.stdout.is_empty());
}
