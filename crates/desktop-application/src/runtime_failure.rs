//! Runtime transport failure facts used by read-only recovery and diagnostics.
use mixrelay_control_protocol::{ControlFailureCode, ControlStage, FailureDelivery};
use std::io;

/// Native causes stay in adapters; policy consumes only their delivery and I/O facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeFailure {
    HomeDirectory,
    HomeMetadata,
    ControlDirectory,
    ConnectTimeout,
    Connect(io::ErrorKind),
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
}
impl RuntimeFailure {
    #[must_use]
    pub fn stage(&self) -> ControlStage {
        match self {
            Self::HomeDirectory | Self::HomeMetadata | Self::ControlDirectory => {
                ControlStage::Setup
            }
            Self::ConnectTimeout | Self::Connect(_) => ControlStage::Connect,
            Self::PeerCredentials | Self::PeerMismatch => ControlStage::PeerVerification,
            Self::ClosedBeforeAdmission => ControlStage::Admission,
            Self::RequestTooLarge => ControlStage::Encode,
            Self::SendFailed => ControlStage::Send,
            Self::DisconnectedAfterAdmission
            | Self::ResponseTimeout
            | Self::ControllerUnavailable => ControlStage::Response,
            Self::InvalidResponse => ControlStage::Decode,
            Self::SubscriptionRejected | Self::SubscriptionTimeout => ControlStage::Subscribe,
        }
    }

    #[must_use]
    pub fn code(&self) -> ControlFailureCode {
        match self {
            Self::HomeDirectory => ControlFailureCode::HomeDirectory,
            Self::HomeMetadata => ControlFailureCode::HomeMetadata,
            Self::ControlDirectory => ControlFailureCode::ControlDirectory,
            Self::ConnectTimeout => ControlFailureCode::ConnectTimeout,
            Self::Connect(_) => ControlFailureCode::ConnectIo,
            Self::PeerCredentials => ControlFailureCode::PeerCredentials,
            Self::PeerMismatch => ControlFailureCode::PeerMismatch,
            Self::ClosedBeforeAdmission => ControlFailureCode::ClosedBeforeAdmission,
            Self::RequestTooLarge => ControlFailureCode::RequestTooLarge,
            Self::SendFailed => ControlFailureCode::SendFailed,
            Self::DisconnectedAfterAdmission => ControlFailureCode::DisconnectedAfterAdmission,
            Self::ResponseTimeout => ControlFailureCode::ResponseTimeout,
            Self::ControllerUnavailable => ControlFailureCode::ControllerUnavailable,
            Self::InvalidResponse => ControlFailureCode::InvalidResponse,
            Self::SubscriptionRejected => ControlFailureCode::SubscriptionRejected,
            Self::SubscriptionTimeout => ControlFailureCode::SubscriptionTimeout,
        }
    }

    #[must_use]
    pub fn delivery(&self) -> FailureDelivery {
        match self {
            Self::HomeDirectory
            | Self::HomeMetadata
            | Self::ControlDirectory
            | Self::ConnectTimeout
            | Self::Connect(_)
            | Self::PeerCredentials
            | Self::PeerMismatch
            | Self::ClosedBeforeAdmission
            | Self::RequestTooLarge
            | Self::SendFailed => FailureDelivery::NotSent,
            Self::DisconnectedAfterAdmission
            | Self::ResponseTimeout
            | Self::ControllerUnavailable
            | Self::InvalidResponse
            | Self::SubscriptionRejected
            | Self::SubscriptionTimeout => FailureDelivery::Uncertain,
        }
    }

    /// This classification permits only another catalog read. It never authorizes a mutation.
    #[must_use]
    pub fn catalog_read_retryable(&self) -> bool {
        match self {
            Self::ConnectTimeout
            | Self::ClosedBeforeAdmission
            | Self::SendFailed
            | Self::DisconnectedAfterAdmission
            | Self::ResponseTimeout
            | Self::ControllerUnavailable => true,
            Self::Connect(kind) => matches!(
                kind,
                io::ErrorKind::NotFound
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::TimedOut
            ),
            _ => false,
        }
    }
}
