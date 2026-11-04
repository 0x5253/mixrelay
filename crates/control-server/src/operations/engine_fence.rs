//! Engine identity: every reply names the engine incarnation, and a request expecting
//! another incarnation is refused before it can consume ingress or admission.
use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};
use mixrelay_control_protocol::ErrorCode;

use super::{dispatch::needs_operation, responses::not_admitted};

pub(super) const ENGINE_HEADER: &str = "x-mixrelay-engine";
const EXPECTED_ENGINE_HEADER: &str = "x-mixrelay-expected-engine";

/// Converts a backend incarnation into the response header once, at router construction.
/// An empty incarnation means the backend has no identity; an incarnation that cannot be a
/// header value is reported and treated the same, so clients never receive a fence for it.
pub(crate) fn engine_header(incarnation: String) -> Option<HeaderValue> {
    if incarnation.is_empty() {
        return None;
    }
    match HeaderValue::try_from(incarnation) {
        Ok(header) => Some(header),
        Err(error) => {
            tracing::error!(%error, "engine incarnation is not a valid header value");
            None
        }
    }
}

/// Fence before ingress/admission and stamp every response, including rejections.
pub(crate) async fn engine_identity(
    State(engine): State<Option<HeaderValue>>,
    request: Request,
    next: Next,
) -> Response {
    let incarnation = engine.as_ref().map_or(&b""[..], HeaderValue::as_bytes);
    let mismatch = needs_operation(&request)
        && request
            .headers()
            .get(EXPECTED_ENGINE_HEADER)
            .is_some_and(|expected| expected.as_bytes() != incarnation);
    let mut response = if mismatch {
        not_admitted(StatusCode::CONFLICT, ErrorCode::EngineReplaced)
    } else {
        next.run(request).await
    };
    if let Some(engine) = engine {
        response.headers_mut().insert(ENGINE_HEADER, engine);
    }
    response
}

#[cfg(test)]
mod tests;
