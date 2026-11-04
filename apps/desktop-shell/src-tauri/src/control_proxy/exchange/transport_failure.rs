//! A reqwest failure with the exchange stage it interrupted, translated into the
//! platform-neutral failure report.
use std::error::Error as _;

use mixrelay_control_protocol::ControlStage;
use mixrelay_desktop_core::engine_control::failure::{ExchangeFailure, TransportFault};

#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub(super) struct TransportFailure {
    stage: ControlStage,
    #[source]
    source: reqwest::Error,
}

impl TransportFailure {
    /// A failure while sending the request itself.
    pub(super) fn request(source: reqwest::Error) -> Self {
        let stage = fault(&source).request_stage();
        Self { stage, source }
    }

    pub(super) fn at(stage: ControlStage, source: reqwest::Error) -> Self {
        Self { stage, source }
    }

    pub(super) fn report<'a>(
        &self,
        path: &'a str,
        may_have_been_delivered: bool,
    ) -> ExchangeFailure<'a> {
        ExchangeFailure {
            path,
            reason: self.to_string(),
            fault: fault(&self.source),
            stage: self.stage,
            platform: crate::control_channel::failure_platform(),
            os_error: self.os_error(),
            may_have_been_delivered,
        }
    }

    /// The first OS error in the cause chain.
    fn os_error(&self) -> Option<i32> {
        let mut cause = self.source.source();
        while let Some(error) = cause {
            if let Some(error) = error.downcast_ref::<std::io::Error>() {
                return error.raw_os_error();
            }
            cause = error.source();
        }
        None
    }
}

fn fault(error: &reqwest::Error) -> TransportFault {
    TransportFault {
        connect: error.is_connect(),
        timeout: error.is_timeout(),
        body: error.is_body(),
    }
}
