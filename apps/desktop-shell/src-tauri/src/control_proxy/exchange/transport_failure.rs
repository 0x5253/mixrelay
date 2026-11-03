//! HTTP control failures retain the procedure that failed and request delivery uncertainty.
use std::error::Error as _;

use mixrelay_control_protocol::{
    ControlBoundary, ControlFailureCode, ControlFailureContext, ControlStage, FailureDelivery,
};

#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub(super) struct TransportFailure {
    stage: ControlStage,
    #[source]
    source: reqwest::Error,
}

impl TransportFailure {
    pub(super) fn request(source: reqwest::Error) -> Self {
        let stage = if source.is_connect() {
            ControlStage::Connect
        } else {
            ControlStage::Request
        };
        Self { stage, source }
    }

    pub(super) fn at(stage: ControlStage, source: reqwest::Error) -> Self {
        Self { stage, source }
    }

    pub(super) fn is_connect(&self) -> bool {
        self.source.is_connect()
    }

    pub(super) fn unavailable(&self) -> bool {
        self.source.is_connect() || self.source.is_timeout()
    }

    pub(super) fn diagnostic(&self, may_have_been_delivered: bool) -> ControlFailureContext {
        let failure = if self.source.is_timeout() {
            ControlFailureCode::Timeout
        } else if self.source.is_connect() {
            ControlFailureCode::ConnectIo
        } else if self.source.is_body() {
            ControlFailureCode::BodyIo
        } else {
            ControlFailureCode::TransportIo
        };
        let mut cause = self.source.source();
        let mut os_error = None;
        while let Some(error) = cause {
            if let Some(error) = error.downcast_ref::<std::io::Error>() {
                os_error = error.raw_os_error();
                break;
            }
            cause = error.source();
        }
        ControlFailureContext {
            platform: crate::control_channel::failure_platform(),
            boundary: ControlBoundary::RuntimeEngine,
            stage: self.stage,
            failure,
            delivery: if !may_have_been_delivered && self.is_connect() {
                FailureDelivery::NotSent
            } else {
                FailureDelivery::Uncertain
            },
            os_error,
        }
    }
}
