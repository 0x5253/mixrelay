//! Sends control commands once and resolves accepted operation receipts.
use std::{
    collections::hash_map::RandomState,
    hash::BuildHasher,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use mixrelay_control_protocol::ControlStage;
use reqwest::{Method, Response};
mod rejection;
mod transport_failure;
use serde_json::Value;
use transport_failure::TransportFailure;

#[cfg(target_os = "macos")]
use super::RequestDelivery;
use super::{ControlProxyClient, ControlProxyResponse};

const OPERATION_HEADER: &str = "x-mixrelay-operation";

// The caller retains its admission guard across polling. An accepted mutation is
// submitted exactly once, even when later observation fails.
pub(super) async fn request(
    client: &ControlProxyClient,
    origin: &str,
    method: Method,
    path: &str,
    mut body: Option<Value>,
) -> Result<ControlProxyResponse, String> {
    let supplied = body
        .as_mut()
        .and_then(Value::as_object_mut)
        .and_then(|body| body.remove("clientOperationId"))
        .and_then(|id| id.as_str().map(str::to_owned))
        .filter(|_| !path.ends_with("/preview"));
    let requested =
        needs_operation(&method, path).then(|| supplied.unwrap_or_else(new_operation_id));
    if method == Method::GET && path.starts_with("/api/v1/operations/") {
        return observe_operation(client, origin, path).await;
    }
    let mut request = client
        .client
        .request(method, format!("{origin}{path}"))
        .timeout(Duration::from_secs(30));
    if let Some(id) = &requested {
        request = request.header("x-mixrelay-operation-request", id);
        if let Some(engine) = client
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            request = request.header("x-mixrelay-expected-engine", engine);
        }
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let result = match request.send().await {
        Ok(response) => finish(client, origin, response, requested.as_deref(), None).await,
        Err(error) if error.is_connect() => {
            return Ok(failed(path, &TransportFailure::request(error), false));
        }
        Err(error) => Err(ExchangeError::Transport(TransportFailure::request(error))),
    };
    match result {
        Ok(response) => Ok(response),
        Err(ExchangeError::Invalid(reason)) => Err(reason),
        Err(ExchangeError::Transport(error)) => {
            let Some(id) = requested.as_deref() else {
                return Ok(failed(path, &error, true));
            };
            // Headers and even a partial body can arrive before transport fails.
            // Resolve once, including failures while polling; never resubmit work.
            let resolved = client
                .client
                .get(operation_url(origin, id)?)
                .timeout(Duration::from_secs(5))
                .send()
                .await;
            let Ok(response) = resolved else {
                return Ok(failed(path, &error, true));
            };
            let engine = observe_engine(client, &response);
            // A stored terminal 404 is a valid result. A missing receipt proves
            // nothing: the engine may have restarted since admitting the request.
            if validate_terminal(&response, Some(id)).is_err()
                && operation_id(&response).ok().flatten().as_deref() != Some(id)
            {
                let mut result = failed(path, &error, true);
                result.engine = engine;
                return Ok(result);
            }
            match finish(client, origin, response, Some(id), Some(id.to_owned())).await {
                Ok(response) => Ok(response),
                Err(ExchangeError::Invalid(reason)) => Err(reason),
                Err(ExchangeError::Transport(error)) => Ok(failed(path, &error, true)),
            }
        }
    }
}

// Read-only receipt observation does not consume or acknowledge the writer's result.
async fn observe_operation(
    client: &ControlProxyClient,
    origin: &str,
    path: &str,
) -> Result<ControlProxyResponse, String> {
    let response = client
        .client
        .get(format!("{origin}{path}"))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let engine = observe_engine(client, &response);
    let status = response.status().as_u16();
    let body = response.json().await.map_err(|error| error.to_string())?;
    Ok(ControlProxyResponse {
        engine,
        status,
        body,
        #[cfg(target_os = "macos")]
        delivery: RequestDelivery::ServerResponse,
    })
}

enum ExchangeError {
    Transport(TransportFailure),
    Invalid(String),
}

impl From<String> for ExchangeError {
    fn from(reason: String) -> Self {
        Self::Invalid(reason)
    }
}

async fn finish(
    client: &ControlProxyClient,
    origin: &str,
    mut response: Response,
    requested: Option<&str>,
    mut accepted: Option<String>,
) -> Result<ControlProxyResponse, ExchangeError> {
    let mut response_stage = if accepted.is_some() {
        ControlStage::ReceiptObservation
    } else {
        ControlStage::Response
    };
    loop {
        let engine = observe_engine(client, &response);
        let pending = operation_id(&response)?;
        // Fast client-ID completions are retained too and require acknowledgement.
        let completed = response
            .headers()
            .get("x-mixrelay-operation-result")
            .and_then(|header| header.to_str().ok())
            .map(str::to_owned);
        match &pending {
            None => {
                validate_terminal(&response, accepted.as_deref())?;
                // A result for any other operation is neither this request's outcome
                // nor ours to acknowledge.
                if completed.is_some() && completed.as_deref() != requested {
                    return Err(IDENTITY_CHANGED.to_owned().into());
                }
            }
            // The engine adopts the client's ID for every receipt; any other ID is
            // not this request. Record it before reading the body so a lost body is
            // resolved through the receipt and later results must bear it.
            Some(id) if requested == Some(id.as_str()) => {
                accepted = Some(id.clone());
            }
            Some(_) => return Err(IDENTITY_CHANGED.to_owned().into()),
        }
        let status = response.status().as_u16();
        let body: Value = if status == 204 {
            Value::Null
        } else {
            // Reqwest classifies some body transport errors as decode errors too.
            // Separate byte reads from JSON parsing to preserve that distinction.
            let bytes = response.bytes().await.map_err(|source| {
                ExchangeError::Transport(TransportFailure::at(response_stage, source))
            })?;
            serde_json::from_slice(&bytes).map_err(|error| {
                ExchangeError::Invalid(format!("local control response was invalid: {error}"))
            })?
        };
        let Some(id) = pending else {
            rejection::observe(status, &body);
            if let Some(id) = completed {
                acknowledge(client, &operation_url(origin, &id)?).await;
            }
            return Ok(ControlProxyResponse {
                engine,
                status,
                body,
                #[cfg(target_os = "macos")]
                delivery: RequestDelivery::ServerResponse,
            });
        };
        validate_receipt(&id, &body)?;
        client.operation_liveness.notify_waiters();
        let url = operation_url(origin, &id)?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        response_stage = ControlStage::ReceiptObservation;
        response = match client
            .client
            .get(url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                return Err(ExchangeError::Transport(TransportFailure::at(
                    ControlStage::ReceiptObservation,
                    error,
                )));
            }
        };
    }
}

pub(super) fn observe_engine(client: &ControlProxyClient, response: &Response) -> Option<String> {
    let engine = response
        .headers()
        .get("x-mixrelay-engine")
        .and_then(|header| header.to_str().ok())
        .map(str::to_owned);
    if let Some(engine) = &engine {
        *client
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(engine.clone());
    }
    engine
}

fn new_operation_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let random = RandomState::new();
    format!(
        "{:016x}{:016x}",
        random.hash_one((now, sequence, 0)),
        random.hash_one((now, sequence, 1))
    )
}

// Match the engine operation dispatcher, including its potentially blocking reads.
fn needs_operation(method: &Method, path: &str) -> bool {
    if path.starts_with("/api/v1/operations/")
        || path == "/api/v1/operations"
        || matches!(
            path,
            "/health"
                | "/api/v1/status"
                | "/api/v1/graph/status"
                | "/api/v1/controls/snapshot"
                | "/api/v1/meters"
        )
        || path.ends_with("/editor/frame")
        || path.ends_with("/editor/input")
    {
        return false;
    }
    if method == Method::GET {
        return matches!(
            path,
            "/api/v1/graph"
                | "/api/v1/graphs"
                | "/api/v1/processors"
                | "/api/v1/application-capture/manifest"
        ) || path.starts_with("/api/v1/graphs/");
    }
    method == Method::POST || method == Method::PUT
}

fn validate_terminal(response: &Response, accepted: Option<&str>) -> Result<(), String> {
    let Some(accepted) = accepted else {
        return Ok(());
    };
    if response
        .headers()
        .get("x-mixrelay-operation-result")
        .and_then(|header| header.to_str().ok())
        != Some(accepted)
    {
        return Err("local control operation result could not be confirmed".to_owned());
    }
    Ok(())
}

fn operation_id(response: &Response) -> Result<Option<String>, String> {
    let Some(header) = response.headers().get(OPERATION_HEADER) else {
        return Ok(None);
    };
    let id = header.to_str().map_err(|_| "invalid operation receipt")?;
    if response.status() != 202 || id.is_empty() || id.len() > 128 {
        return Err("invalid operation receipt".to_owned());
    }
    Ok(Some(id.to_owned()))
}

const IDENTITY_CHANGED: &str = "local control operation identity changed or was invalid";

fn validate_receipt(id: &str, body: &Value) -> Result<(), String> {
    if body.get("operationId").and_then(Value::as_str) != Some(id) {
        return Err(IDENTITY_CHANGED.to_owned());
    }
    Ok(())
}

fn operation_url(origin: &str, id: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(origin).map_err(|error| error.to_string())?;
    url.path_segments_mut()
        .map_err(|()| "invalid control origin".to_owned())?
        .extend(["api", "v1", "operations", id]);
    Ok(url)
}

async fn acknowledge(client: &ControlProxyClient, url: &reqwest::Url) {
    // A lost acknowledgement only delays server-side receipt reclamation. It
    // must never replace an already observed transaction result with an error.
    match client
        .client
        .delete(url.clone())
        .timeout(Duration::from_secs(1))
        .send()
        .await
    {
        Ok(response) => {
            observe_engine(client, &response);
        }
        Err(error) => tracing::debug!(?error, "control receipt acknowledgement failed"),
    }
}

fn failed(path: &str, error: &TransportFailure, accepted: bool) -> ControlProxyResponse {
    let diagnostic = error.diagnostic(accepted);
    tracing::warn!(path, platform = ?diagnostic.platform, boundary = ?diagnostic.boundary,
        stage = ?diagnostic.stage, failure = ?diagnostic.failure, delivery = ?diagnostic.delivery,
        os_error = diagnostic.os_error, "control exchange failed");
    let code = if error.unavailable() {
        "control_unavailable"
    } else {
        "control_request_failed"
    };
    let mut parameters =
        serde_json::json!({"path": path, "reason": error.to_string(), "diagnostic": diagnostic});
    if accepted {
        parameters["operationOutcome"] = Value::from("uncertain");
    }
    ControlProxyResponse {
        engine: None,
        status: 503,
        #[cfg(target_os = "macos")]
        delivery: if !accepted && error.is_connect() {
            RequestDelivery::NotSent
        } else {
            RequestDelivery::Uncertain
        },
        body: serde_json::json!({
            "code": code,
            "parameters": parameters,
        }),
    }
}

#[cfg(test)]
mod tests;
