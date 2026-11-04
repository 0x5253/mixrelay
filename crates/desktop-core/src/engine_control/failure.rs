//! How an exchange that failed in transport is reported to the editor: whether the engine
//! may have the request, whether it is unavailable, and the structured diagnostic.
use mixrelay_control_protocol::{
    ControlBoundary, ControlFailureCode, ControlFailureContext, ControlPlatform, ControlStage,
    FailureDelivery,
};
use serde_json::{Value, json};

use super::Delivery;

/// What the transport reported about a failure. One error can be several of these, such
/// as a connect attempt that timed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransportFault {
    pub connect: bool,
    pub timeout: bool,
    pub body: bool,
}

impl TransportFault {
    /// The stage of a failure while sending the request itself.
    #[must_use]
    pub fn request_stage(self) -> ControlStage {
        if self.connect {
            ControlStage::Connect
        } else {
            ControlStage::Request
        }
    }

    fn code(self) -> ControlFailureCode {
        if self.timeout {
            ControlFailureCode::Timeout
        } else if self.connect {
            ControlFailureCode::ConnectIo
        } else if self.body {
            ControlFailureCode::BodyIo
        } else {
            ControlFailureCode::TransportIo
        }
    }

    /// No connection or no answer in time: the engine is unavailable rather than failing.
    fn unavailable(self) -> bool {
        self.connect || self.timeout
    }
}

/// One failed exchange with the engine.
#[derive(Debug, Clone)]
pub struct ExchangeFailure<'a> {
    pub path: &'a str,
    pub reason: String,
    pub fault: TransportFault,
    pub stage: ControlStage,
    pub platform: ControlPlatform,
    pub os_error: Option<i32>,
    /// The engine may have admitted the request: it was sent, or a receipt was issued.
    pub may_have_been_delivered: bool,
}

impl ExchangeFailure<'_> {
    /// Only a refused connection for a request that was never accepted proves non-delivery.
    #[must_use]
    pub fn delivery(&self) -> Delivery {
        if !self.may_have_been_delivered && self.fault.connect {
            Delivery::NotSent
        } else {
            Delivery::Uncertain
        }
    }

    #[must_use]
    pub fn diagnostic(&self) -> ControlFailureContext {
        ControlFailureContext {
            platform: self.platform,
            boundary: ControlBoundary::RuntimeEngine,
            stage: self.stage,
            failure: self.fault.code(),
            delivery: match self.delivery() {
                Delivery::NotSent => FailureDelivery::NotSent,
                Delivery::ServerResponse | Delivery::Uncertain => FailureDelivery::Uncertain,
            },
            os_error: self.os_error,
        }
    }

    /// The error body the editor receives with status 503.
    #[must_use]
    pub fn body(&self) -> Value {
        let code = if self.fault.unavailable() {
            "control_unavailable"
        } else {
            "control_request_failed"
        };
        let mut parameters = json!({
            "path": self.path,
            "reason": self.reason,
            "diagnostic": self.diagnostic(),
        });
        if self.may_have_been_delivered {
            parameters["operationOutcome"] = Value::from("uncertain");
        }
        json!({"code": code, "parameters": parameters})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(fault: TransportFault, may_have_been_delivered: bool) -> ExchangeFailure<'static> {
        ExchangeFailure {
            path: "/api/v1/graph",
            reason: "connection refused".into(),
            fault,
            stage: fault.request_stage(),
            platform: ControlPlatform::MacOs,
            os_error: Some(61),
            may_have_been_delivered,
        }
    }

    const CONNECT: TransportFault = TransportFault {
        connect: true,
        timeout: false,
        body: false,
    };

    #[test]
    fn only_a_refused_connection_before_acceptance_proves_the_request_was_not_sent() {
        assert_eq!(failure(CONNECT, false).delivery(), Delivery::NotSent);
        assert_eq!(failure(CONNECT, true).delivery(), Delivery::Uncertain);
        for fault in [
            TransportFault {
                timeout: true,
                ..TransportFault::default()
            },
            TransportFault {
                body: true,
                ..TransportFault::default()
            },
            TransportFault::default(),
        ] {
            assert_eq!(failure(fault, false).delivery(), Delivery::Uncertain);
        }
    }

    #[test]
    fn a_refused_connection_reports_an_unavailable_engine_that_never_saw_the_request() {
        let body = failure(CONNECT, false).body();
        assert_eq!(
            body,
            json!({"code":"control_unavailable","parameters":{
                "path":"/api/v1/graph",
                "reason":"connection refused",
                "diagnostic":{
                    "platform":"macos",
                    "boundary":"runtime_engine",
                    "stage":"connect",
                    "failure":"connect_io",
                    "delivery":"not_sent",
                    "osError":61,
                },
            }})
        );
    }

    #[test]
    fn a_possibly_delivered_request_is_reported_with_an_uncertain_outcome() {
        let mut lost = failure(
            TransportFault {
                body: true,
                ..TransportFault::default()
            },
            true,
        );
        lost.stage = ControlStage::ReceiptObservation;
        lost.os_error = None;
        let body = lost.body();
        assert_eq!(body["code"], "control_request_failed");
        assert_eq!(body["parameters"]["operationOutcome"], "uncertain");
        assert_eq!(
            body["parameters"]["diagnostic"]["stage"],
            "receipt_observation"
        );
        assert_eq!(body["parameters"]["diagnostic"]["failure"], "body_io");
        assert_eq!(body["parameters"]["diagnostic"]["delivery"], "uncertain");
        assert!(body["parameters"]["diagnostic"].get("osError").is_none());
    }

    #[test]
    fn a_timed_out_connect_is_an_unavailable_timeout_that_was_not_sent() {
        let fault = TransportFault {
            connect: true,
            timeout: true,
            body: false,
        };
        let timed_out = failure(fault, false);
        assert_eq!(timed_out.delivery(), Delivery::NotSent);
        let body = timed_out.body();
        assert_eq!(body["code"], "control_unavailable");
        assert_eq!(body["parameters"]["diagnostic"]["failure"], "timeout");
        assert_eq!(body["parameters"]["diagnostic"]["stage"], "connect");
        assert!(body["parameters"].get("operationOutcome").is_none());
    }

    #[test]
    fn a_failure_after_sending_is_a_request_failure_not_unavailability() {
        let fault = TransportFault::default();
        assert_eq!(fault.request_stage(), ControlStage::Request);
        let body = failure(fault, true).body();
        assert_eq!(body["code"], "control_request_failed");
        assert_eq!(body["parameters"]["diagnostic"]["failure"], "transport_io");
    }
}
