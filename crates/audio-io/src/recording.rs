//! Recording commands and observed take state shared by engine and recorder adapters.

use serde::{Deserialize, Serialize};

/// WAV sample encoding, fixed for the lifetime of a take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Encoding {
    /// Signed sixteen-bit PCM, with final-stage TPDF dither.
    Pcm16,
    /// Packed signed twenty-four-bit PCM, with final-stage TPDF dither.
    Pcm24,
    /// IEEE single precision PCM, without quantization or dither.
    Float32,
}

/// A recording control action. Record resumes a paused file without adding silence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecorderAction {
    /// Create a new take, or resume the current paused take.
    Record,
    /// Drain accepted PCM and pause, retaining the open file.
    Pause,
    /// Drain, flush conversion tails and finalize the WAV header.
    Stop,
}

/// Control-thread request. A new take requires an absolute, nonexistent path.
/// Omitted output rate follows the most recently observed input rate; omitted
/// encoding selects PCM24. Resume cannot change the existing take's settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecorderRequest {
    /// Requested state transition.
    pub action: RecorderAction,
    /// Destination of a new take; must not overwrite an existing file.
    pub path: Option<String>,
    /// Fixed output sample rate in Hz, independent of subsequent input changes.
    pub sample_rate: Option<u32>,
    /// Fixed WAV sample encoding.
    pub encoding: Option<Encoding>,
}

/// Writer lifecycle, not merely the callback's admission flag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecorderState {
    /// No open writer; the most recent take has finalized.
    #[default]
    Stopped,
    /// New callback packets are accepted.
    Recording,
    /// File remains open, but no callback PCM is accepted.
    Paused,
    /// Recorder-local error; audio graph processing is unaffected.
    Failed,
}

/// Control-visible state. Frames count output frames actually written, excluding
/// pauses, in the take’s single RF64 file. A stopped result is returned
/// only after WAV finalization succeeds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderStatus {
    /// Current writer state.
    pub state: RecorderState,
    /// Absolute take destination, retained across pause/resume.
    pub path: Option<String>,
    /// Fixed output rate for the take.
    pub sample_rate: Option<u32>,
    /// Fixed output encoding for the take.
    pub encoding: Option<Encoding>,
    /// Complete output frames written so far in this take.
    pub frames: u64,
    /// Recorder-local error, retained until a new take starts successfully.
    pub error: Option<String>,
}
