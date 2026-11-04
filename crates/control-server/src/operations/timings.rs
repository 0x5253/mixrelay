use std::time::Duration;

/// Time bounds of one operation owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationTimings {
    /// How long a client may take to send a request body before ingress rejects it.
    pub body_deadline: Duration,
    /// How long a mutation may run before its reply becomes a pollable pending operation.
    pub completion_wait: Duration,
}

impl Default for OperationTimings {
    /// The product bounds: a two-second body deadline and a 100 ms completion wait.
    fn default() -> Self {
        Self {
            body_deadline: Duration::from_secs(2),
            completion_wait: Duration::from_millis(100),
        }
    }
}
