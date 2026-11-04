//! What a submission's answer means for the application captures reserved for it. Taps are
//! reserved before the submission is sent, so a lost reply or cancelled command cannot
//! destroy capture the engine may already be using.
use serde_json::Value;

use super::Delivery;

/// Appended to errors that leave reserved captures in place.
pub const CAPTURES_RETAINED: &str = "application captures retained until engine confirmation";

/// A mutation that may change which applications the engine captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submission {
    Graph,
    Workspace,
    Power,
    NodeControl,
}

impl Submission {
    /// The revision the engine reports having accepted.
    fn accepted_revision(self, body: &Value) -> Option<u64> {
        let acknowledgement = match self {
            Self::Workspace => &body["document"],
            Self::NodeControl => &body["application"],
            Self::Graph | Self::Power => body,
        };
        acknowledgement.get("revision").and_then(Value::as_u64)
    }
}

/// What happens to the reserved captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settlement {
    /// The engine certainly did not apply the submission: release the reservation.
    Release,
    /// The outcome is unknown: keep the reservation until the engine reports a revision.
    Retain,
    /// The engine accepted the prepared revision: the reservation becomes active.
    Accept,
}

/// The engine accepted a revision other than the one the captures were prepared for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("graph acceptance does not match prepared capture revision")]
pub struct RevisionMismatch;

/// Settles the reservation for `prepared_revision` from the submission's answer.
///
/// # Errors
/// Returns [`RevisionMismatch`] for a success that acknowledges another revision; the
/// reservation is then retained.
pub fn settle(
    submission: Submission,
    delivery: Delivery,
    status: u16,
    body: &Value,
    prepared_revision: u64,
) -> Result<Settlement, RevisionMismatch> {
    if definitively_rejected(delivery, status, body) {
        return Ok(Settlement::Release);
    }
    if !(200..300).contains(&status) {
        return Ok(Settlement::Retain);
    }
    if submission.accepted_revision(body) != Some(prepared_revision) {
        return Err(RevisionMismatch);
    }
    Ok(Settlement::Accept)
}

/// Marks a retained error answer so the editor knows capture awaits confirmation.
pub fn mark_pending_confirmation(body: &mut Value) {
    if let Some(parameters) = body.get_mut("parameters").and_then(Value::as_object_mut) {
        parameters.insert(
            "captureOutcome".into(),
            Value::String("pending_confirmation".into()),
        );
    }
}

/// Unsent, a client error, or an explicit "not applied" outcome. A lost or uncertain
/// reply never is.
fn definitively_rejected(delivery: Delivery, status: u16, body: &Value) -> bool {
    let parameters = &body["parameters"];
    match delivery {
        Delivery::NotSent => true,
        Delivery::Uncertain => false,
        Delivery::ServerResponse => {
            (400..500).contains(&status)
                || ["workspaceOutcome", "powerOutcome", "nodeControlOutcome"]
                    .into_iter()
                    .any(|field| parameters[field] == "not_applied")
        }
    }
}

#[cfg(test)]
mod tests;
