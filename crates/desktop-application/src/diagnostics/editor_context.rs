//! Editor facts a user includes when saving a support report from the diagnostics window.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The editor reports a handful of current problems; more means a faulty caller.
const PROBLEM_LIMIT: usize = 32;
/// Problem codes are identifiers such as `graph_apply_timeout`, never free text.
const CODE_LIMIT: usize = 64;
/// One entry per plug-in format and scan stage.
const SCAN_GROUP_LIMIT: usize = 10;

/// Current editor problems and scan-failure counts. Names, paths and messages never cross.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawEditorContext")]
pub struct EditorContext {
    problems: Vec<ProblemCode>,
    processor_scan: Vec<ScanFailures>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawEditorContext {
    problems: Vec<ProblemCode>,
    processor_scan: Vec<ScanFailures>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
struct ProblemCode(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ProcessorFormat {
    Au,
    Auv3,
    Clap,
    Lv2,
    Vst3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ScanStage {
    Scan,
    Probe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScanFailures {
    format: ProcessorFormat,
    stage: ScanStage,
    count: u32,
}

/// Why an editor payload was refused before any file dialog opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditorContextError {
    #[error("problem code is not a short identifier")]
    Code,
    #[error("editor context exceeds its fixed limits")]
    Limit,
}

impl TryFrom<String> for ProblemCode {
    type Error = EditorContextError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let identifier = !value.is_empty()
            && value.len() <= CODE_LIMIT
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        identifier
            .then_some(Self(value))
            .ok_or(EditorContextError::Code)
    }
}

impl TryFrom<RawEditorContext> for EditorContext {
    type Error = EditorContextError;

    fn try_from(raw: RawEditorContext) -> Result<Self, Self::Error> {
        if raw.problems.len() > PROBLEM_LIMIT || raw.processor_scan.len() > SCAN_GROUP_LIMIT {
            return Err(EditorContextError::Limit);
        }
        Ok(Self {
            problems: raw.problems,
            processor_scan: raw.processor_scan,
        })
    }
}

impl EditorContext {
    pub(crate) fn to_report(&self) -> Value {
        json!({
            "problems": self.problems.iter().map(|code| code.0.as_str()).collect::<Vec<_>>(),
            "processorScan": self.processor_scan,
        })
    }
}

#[cfg(test)]
mod tests;
