//! Native capture errors retain the platform, caller operation and original OS status.

use mixrelay_macos_process_tap::TapError;
use std::fmt;

pub(super) const NATIVE_OK: u32 = 0;
pub(super) const NATIVE_INVALID_ARGUMENT: u32 = 1;
pub(super) const NATIVE_RESOURCE_BUSY: u32 = 9;

#[derive(Clone, Copy, Debug)]
pub(super) enum CaptureOperation {
    Validate,
    Prepare,
    Start,
    Stop,
    Refresh,
    Enumerate,
}

impl CaptureOperation {
    const fn name(self) -> &'static str {
        match self {
            Self::Validate => "validate",
            Self::Prepare => "prepare",
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Refresh => "refresh",
            Self::Enumerate => "enumerate",
        }
    }
}

#[derive(Debug)]
pub(super) struct CaptureProbeError {
    operation: CaptureOperation,
    native: TapError,
}

impl CaptureProbeError {
    pub(super) const fn new(operation: CaptureOperation, native: TapError) -> Self {
        Self { operation, native }
    }
}

impl fmt::Display for CaptureProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "macOS application capture {} failed: {} (OSStatus {})",
            self.operation.name(),
            native_error_name(self.native.code),
            self.native.os_status
        )
    }
}

impl std::error::Error for CaptureProbeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.native)
    }
}

fn native_error_name(code: u32) -> &'static str {
    match code {
        NATIVE_OK => "ok",
        NATIVE_INVALID_ARGUMENT => "invalid_argument",
        2 => "unsupported_system",
        3 => "application_not_found",
        4 => "core_audio_failure",
        5 => "unsupported_format",
        6 => "out_of_memory",
        7 => "already_running",
        8 => "not_running",
        NATIVE_RESOURCE_BUSY => "resource_busy",
        _ => "unknown_native_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_native_errors_identify_each_capture_operation() {
        let native = TapError {
            code: 4,
            os_status: -10863,
        };
        for (operation, expected) in [
            (
                CaptureOperation::Prepare,
                "macOS application capture prepare failed: core_audio_failure (OSStatus -10863)",
            ),
            (
                CaptureOperation::Start,
                "macOS application capture start failed: core_audio_failure (OSStatus -10863)",
            ),
            (
                CaptureOperation::Stop,
                "macOS application capture stop failed: core_audio_failure (OSStatus -10863)",
            ),
            (
                CaptureOperation::Refresh,
                "macOS application capture refresh failed: core_audio_failure (OSStatus -10863)",
            ),
        ] {
            assert_eq!(
                CaptureProbeError::new(operation, native).to_string(),
                expected
            );
        }
        assert_eq!(size_of::<TapError>(), 8);
    }
}
