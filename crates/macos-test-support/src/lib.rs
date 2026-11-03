//! Cargo-owned native contract runner, used only as a macOS dev-dependency. Native C
//! tests exercise the actual FFI modules with sanitizers, in owned bounded processes.
#![cfg(target_os = "macos")]
use std::{process::Command, time::Duration};

mod process;

const NATIVE_TIMEOUT: Duration = Duration::from_mins(1);

/// Runs native callback and HAL assertions with `ASan` and `UBSan` enabled.
/// # Panics
/// A native assertion, sanitizer diagnostic or launch failure fails the test.
pub fn assert_native_contracts() {
    for path in [
        env!("MR_NATIVE_CLOCK"),
        env!("MR_NATIVE_HAL"),
        env!("MR_NATIVE_APPLICATION_CLOCK"),
        env!("MR_NATIVE_AUDIO_RING"),
        env!("MR_NATIVE_PCM_RING"),
    ] {
        // UBSan normally recovers after diagnostics; tests must instead fail on
        // the first violation, just as ASan and native assertions do.
        let result = process::run(
            Command::new(path)
                .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
                .env("ASAN_OPTIONS", "halt_on_error=1"),
            NATIVE_TIMEOUT,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        eprintln!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            result.status.success(),
            "native test failed: {path}: {}",
            result.status
        );
    }
}

/// `sizeof(MRIOTiming)` and the offsets of its `now`, `input` and `output` fields, as the
/// engine's C header declares the callback timing.
/// # Panics
/// A probe launch failure, failed exit or malformed report fails the test.
#[must_use]
pub fn device_io_timing_layout() -> [usize; 4] {
    let result = process::run(
        &mut Command::new(env!("MR_NATIVE_TIMING_LAYOUT")),
        NATIVE_TIMEOUT,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(result.status.success(), "timing layout probe failed");
    let report = String::from_utf8_lossy(&result.stdout);
    let values = report
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<Vec<usize>, _>>()
        .unwrap_or_else(|error| panic!("malformed timing layout report {report:?}: {error}"));
    values
        .try_into()
        .unwrap_or_else(|values| panic!("expected four timing layout values, got {values:?}"))
}
