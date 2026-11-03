//! Errors surfaced by the host framework.
//!
//! Format wrappers also produce these; a CLAP-specific status
//! comes back as `Error::Format { format: "clap", code, message }`
//! rather than its own variant, so consumers can match on
//! "did anything fail?" without enumerating per-format codes.

use std::path::PathBuf;

/// Result alias for rack operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Bounded failures reported by native processing without allocating a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProcessFault {
    /// Input events exceed storage prepared during activation.
    #[error("input event capacity exhausted")]
    InputEventCapacity,
    /// Input event data is outside the format's valid range or block extent.
    #[error("invalid input event")]
    InvalidInputEvent,
    /// Native output exceeds the host's prepared event storage.
    #[error("output event capacity exhausted")]
    OutputEventCapacity,
    /// Native output has an invalid header, length, type or timestamp.
    #[error("invalid output event")]
    InvalidOutputEvent,
    /// The active audio layout cannot address the supplied channel.
    #[error("audio mapping unavailable")]
    InvalidAudioMapping,
    /// The requested block exceeds the activated frame extent.
    #[error("invalid process block size")]
    InvalidBlockSize,
    /// The native instance omitted a required processing callback.
    #[error("required process callback missing")]
    MissingCallback,
    /// Native processing returned a failure status.
    #[error("native processing failed (code {0})")]
    NativeStatus(i64),
}

/// Top-level error type for the host framework.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A realtime processing failure with no allocated diagnostic payload.
    #[error("[{format}] {fault}")]
    Process {
        /// Short identifier for the format wrapper.
        format: &'static str,
        /// Failure observed at the processing boundary.
        fault: ProcessFault,
    },
    /// Plugin not found at the supplied path / id.
    #[error("plugin not found: {0}")]
    PluginNotFound(String),

    /// The plugin was found but failed to load — bad signature,
    /// missing dependency, ABI mismatch.
    #[error("failed to load plugin at {path}: {reason}")]
    LoadFailed {
        /// Path the host tried to load from.
        path: PathBuf,
        /// Format-specific reason ("`clap_plugin_entry` returned null",
        /// "vst3 module-info missing", etc.).
        reason: String,
    },

    /// A plugin call returned a format-specific error code.
    ///
    /// `format` is the wrapper-crate short name (`"clap"`,
    /// `"vst3"`, `"au"`, `"vst2"`, `"lv2"`, `"aax"`).
    #[error("[{format}] {message} (code {code})")]
    Format {
        /// Short identifier for the format wrapper.
        format: &'static str,
        /// Underlying numeric status (`HRESULT` for VST3,
        /// `OSStatus` for AU, etc.).
        code: i64,
        /// Human-readable description from the wrapper.
        message: String,
    },

    /// Parameter index out of range.
    #[error("parameter index {0} out of range")]
    InvalidParameter(usize),

    /// Method invoked before [`crate::PluginCore::activate`].
    #[error("plugin not activated")]
    NotActivated,

    /// The requested activation was rejected, with the instance inactive and reusable.
    /// Adapters may return this only after verifying native cleanup and rollback. Hosts
    /// may omit this configuration during discovery; other failures remain fatal.
    #[error("activation rejected: {reason}")]
    ActivationRejected {
        /// Native rejection and configuration context, retained in discovery diagnostics.
        reason: String,
    },

    /// State blob failed deserialization.
    #[error("state load failed: {0}")]
    StateLoad(#[from] crate::state::StateLoadError),

    /// I/O error during scan / load.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Plugin code panicked across the FFI boundary; caught by
    /// [`crate::wrapper`] helpers and reported as an error rather
    /// than aborting the host.
    #[error("plugin {action} panicked: {message}")]
    Panic {
        /// Which callback was running ("process", "`save_state`", …).
        action: &'static str,
        /// Panic payload extracted as a string.
        message: String,
    },

    /// Catch-all for wrapper-side bugs that don't map to a
    /// format error code. Prefer variants over this when possible.
    #[error("{0}")]
    Other(String),
}
