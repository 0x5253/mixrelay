//! Diagnostic coordinates for failed control transports, independent of user data.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlPlatform {
    #[serde(rename = "macos")]
    MacOs,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlBoundary {
    EditorRuntime,
    RuntimeEngine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlStage {
    Setup,
    Connect,
    PeerVerification,
    Admission,
    Encode,
    Send,
    Request,
    Response,
    Decode,
    Subscribe,
    ReceiptObservation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureDelivery {
    NotSent,
    Uncertain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlFailureCode {
    HomeDirectory,
    HomeMetadata,
    ControlDirectory,
    ConnectTimeout,
    ConnectIo,
    PeerCredentials,
    PeerMismatch,
    ClosedBeforeAdmission,
    RequestTooLarge,
    SendFailed,
    DisconnectedAfterAdmission,
    ResponseTimeout,
    ControllerUnavailable,
    InvalidResponse,
    SubscriptionRejected,
    SubscriptionTimeout,
    Timeout,
    BodyIo,
    TransportIo,
}

/// A failed boundary retains its location and delivery certainty. Names, paths,
/// request arguments and native error messages do not belong in this record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlFailureContext {
    pub platform: ControlPlatform,
    pub boundary: ControlBoundary,
    pub stage: ControlStage,
    pub failure: ControlFailureCode,
    pub delivery: FailureDelivery,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_error: Option<i32>,
}
