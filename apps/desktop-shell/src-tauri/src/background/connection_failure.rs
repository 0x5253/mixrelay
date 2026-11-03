//! Typed editor-to-runtime failures retain the failing stage and delivery uncertainty.
use std::io;

use mixrelay_control_protocol::{ControlBoundary, ControlFailureContext};
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum ConnectionFailure {
    #[error("could not resolve the editor home directory")]
    HomeDirectory(#[source] tauri::Error),
    #[error("could not inspect the editor home directory")]
    HomeMetadata(#[source] io::Error),
    #[error("could not resolve the runtime control directory")]
    ControlDirectory(#[source] io::Error),
    #[error("background connection timed out")]
    ConnectTimeout,
    #[error("could not connect to the background controller: {0}")]
    Connect(#[source] io::Error),
    #[error("could not verify background controller credentials")]
    PeerCredentials(#[source] io::Error),
    #[error("background controller belongs to a different user")]
    PeerMismatch,
    #[error("background connection closed")]
    ClosedBeforeAdmission,
    #[error("background request is too large")]
    RequestTooLarge,
    #[error("background connection unavailable")]
    SendFailed,
    #[error("background disconnected; command was not replayed")]
    DisconnectedAfterAdmission,
    #[error("background response is uncertain; command was not replayed")]
    ResponseTimeout,
    #[error("background controller unavailable")]
    ControllerUnavailable,
    #[error("invalid background result")]
    InvalidResponse,
    #[error("{0}")]
    SubscriptionRejected(String),
    #[error("meter subscription timed out")]
    SubscriptionTimeout,
}

impl ConnectionFailure {
    pub(super) fn diagnostic(&self) -> ControlFailureContext {
        ControlFailureContext {
            platform: crate::control_channel::failure_platform(),
            boundary: ControlBoundary::EditorRuntime,
            stage: self.fact().stage(),
            failure: self.fact().code(),
            delivery: self.fact().delivery(),
            os_error: self.os_error(),
        }
    }

    fn os_error(&self) -> Option<i32> {
        match self {
            Self::HomeMetadata(error)
            | Self::ControlDirectory(error)
            | Self::Connect(error)
            | Self::PeerCredentials(error) => error.raw_os_error(),
            _ => None,
        }
    }

    /// A transient transport loss permits another read, while mutation delivery stays uncertain.
    pub(super) fn catalog_read_retryable(&self) -> bool {
        self.fact().catalog_read_retryable()
    }

    fn fact(&self) -> mixrelay_desktop_core::runtime_failure::RuntimeFailure {
        use mixrelay_desktop_core::runtime_failure::RuntimeFailure as Fact;
        match self {
            Self::HomeDirectory(_) => Fact::HomeDirectory,
            Self::HomeMetadata(_) => Fact::HomeMetadata,
            Self::ControlDirectory(_) => Fact::ControlDirectory,
            Self::ConnectTimeout => Fact::ConnectTimeout,
            Self::Connect(error) => Fact::Connect(error.kind()),
            Self::PeerCredentials(_) => Fact::PeerCredentials,
            Self::PeerMismatch => Fact::PeerMismatch,
            Self::ClosedBeforeAdmission => Fact::ClosedBeforeAdmission,
            Self::RequestTooLarge => Fact::RequestTooLarge,
            Self::SendFailed => Fact::SendFailed,
            Self::DisconnectedAfterAdmission => Fact::DisconnectedAfterAdmission,
            Self::ResponseTimeout => Fact::ResponseTimeout,
            Self::ControllerUnavailable => Fact::ControllerUnavailable,
            Self::InvalidResponse => Fact::InvalidResponse,
            Self::SubscriptionRejected(_) => Fact::SubscriptionRejected,
            Self::SubscriptionTimeout => Fact::SubscriptionTimeout,
        }
    }
}
