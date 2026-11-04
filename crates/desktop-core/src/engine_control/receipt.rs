//! Client operation receipts. A mutation carries a client-chosen operation ID, so its
//! outcome can be resolved by ID after a lost reply instead of being sent again. This
//! module decides which requests carry an ID and what each engine response means for a
//! request that is waiting for its outcome.
use serde_json::Value;

use super::routes;

/// Header carrying a pending operation's receipt ID (`202 Accepted` only).
pub const RECEIPT_HEADER: &str = "x-mixrelay-operation";
/// Header naming the operation whose stored result a response carries.
pub const RESULT_HEADER: &str = "x-mixrelay-operation-result";
/// Request header carrying the client-chosen operation ID.
pub const REQUEST_HEADER: &str = "x-mixrelay-operation-request";
/// Request header fencing a mutation to the engine incarnation the editor observed.
pub const EXPECTED_ENGINE_HEADER: &str = "x-mixrelay-expected-engine";
/// Response header naming the engine incarnation that answered.
pub const ENGINE_HEADER: &str = "x-mixrelay-engine";
/// Body field through which the editor supplies its own operation ID.
const CLIENT_OPERATION_FIELD: &str = "clientOperationId";
const RECEIPT_ID_LIMIT: usize = 128;

/// Why a response cannot be this request's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReceiptError {
    #[error("invalid operation receipt")]
    InvalidReceipt,
    #[error("local control operation result could not be confirmed")]
    Unconfirmed,
    #[error("local control operation identity changed or was invalid")]
    IdentityChanged,
}

/// Whether the engine's operation dispatcher admits a client operation ID for this request.
/// Observation routes, including potentially blocking reads, never carry one.
#[must_use]
pub fn requires_client_operation(method: &str, path: &str) -> bool {
    let observation = beneath(path, routes::OPERATIONS)
        || matches!(
            path,
            "/health"
                | routes::OPERATIONS
                | routes::STATUS
                | routes::GRAPH_STATUS
                | routes::CONTROLS_SNAPSHOT
                | routes::METERS
        )
        || path.ends_with("/editor/frame")
        || path.ends_with("/editor/input");
    if observation {
        return false;
    }
    match method {
        "GET" => {
            matches!(
                path,
                routes::GRAPH
                    | routes::GRAPH_CATALOG
                    | routes::PROCESSORS
                    | routes::CAPTURE_MANIFEST
            ) || beneath(path, routes::GRAPH_CATALOG)
        }
        "POST" | "PUT" => true,
        _ => false,
    }
}

/// A read of a stored receipt. It must not poll or acknowledge the writer's result.
#[must_use]
pub fn is_receipt_observation(method: &str, path: &str) -> bool {
    method == "GET" && beneath(path, routes::OPERATIONS)
}

/// Whether `path` names a resource below the collection `route`.
fn beneath(path: &str, route: &str) -> bool {
    path.strip_prefix(route)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Removes the editor's operation ID from a request body. A preview never adopts it: the
/// ID belongs to the submission that follows the preview.
pub fn take_client_operation_id(body: Option<&mut Value>, path: &str) -> Option<String> {
    let supplied = body?.as_object_mut()?.remove(CLIENT_OPERATION_FIELD)?;
    supplied
        .as_str()
        .filter(|_| !path.ends_with("/preview"))
        .map(str::to_owned)
}

/// The receipt-relevant parts of one engine response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseHeaders<'a> {
    pub status: u16,
    /// The receipt header; pass an unreadable value as `Some("")`, which is invalid.
    pub receipt: Option<&'a str>,
    /// The result header; pass an unreadable value as `None`.
    pub result: Option<&'a str>,
}

/// What the waiting request does with a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// The engine accepted the request under this ID; observe its receipt until it ends.
    Poll(String),
    /// The response is the outcome. Acknowledge the named stored result once its body is
    /// read, so the engine can reclaim it.
    Complete { acknowledge: Option<String> },
}

impl ResponseHeaders<'_> {
    /// Interprets a response to a request that sent `requested` (if any) and whose receipt
    /// `accepted` (if any) the engine already issued.
    ///
    /// # Errors
    /// Rejects malformed receipts, receipts or results for any other operation, and a
    /// terminal response that does not name the accepted operation.
    pub fn next(
        &self,
        requested: Option<&str>,
        accepted: Option<&str>,
    ) -> Result<Next, ReceiptError> {
        // The engine adopts the client's ID for every receipt; any other ID is not this request.
        if let Some(id) = self.receipt()? {
            return if requested == Some(id) {
                Ok(Next::Poll(id.to_owned()))
            } else {
                Err(ReceiptError::IdentityChanged)
            };
        }
        if accepted.is_some() && self.result != accepted {
            return Err(ReceiptError::Unconfirmed);
        }
        // A result for any other operation is neither this request's outcome nor ours to
        // acknowledge. Fast client-ID completions are stored too and need acknowledgement.
        if self.result.is_some() && self.result != requested {
            return Err(ReceiptError::IdentityChanged);
        }
        Ok(Next::Complete {
            acknowledge: self.result.map(str::to_owned),
        })
    }

    /// Whether a receipt lookup after a lost reply found `id`: its stored result (a terminal
    /// 404 included) or its pending receipt. A missing receipt proves nothing, because the
    /// engine may have restarted since admitting the request.
    #[must_use]
    pub fn concerns(&self, id: &str) -> bool {
        self.result == Some(id) || self.receipt().ok().flatten() == Some(id)
    }

    fn receipt(&self) -> Result<Option<&str>, ReceiptError> {
        let Some(id) = self.receipt else {
            return Ok(None);
        };
        if self.status != 202 || id.is_empty() || id.len() > RECEIPT_ID_LIMIT {
            return Err(ReceiptError::InvalidReceipt);
        }
        Ok(Some(id))
    }
}

/// A pending receipt's body must name the same operation as its header.
///
/// # Errors
/// Returns [`ReceiptError::IdentityChanged`] when it does not.
pub fn confirm_receipt_body(id: &str, body: &Value) -> Result<(), ReceiptError> {
    if body.get("operationId").and_then(Value::as_str) == Some(id) {
        Ok(())
    } else {
        Err(ReceiptError::IdentityChanged)
    }
}

#[cfg(test)]
mod tests;
