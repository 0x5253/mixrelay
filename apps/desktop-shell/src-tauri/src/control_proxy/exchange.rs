//! Sends one control request to the engine and resolves its operation receipt. This is the
//! HTTP side only; `engine_control::receipt` decides what each response means.
//!
//! The caller holds admission across polling. An accepted mutation is submitted exactly
//! once, even when a later observation fails.
use std::{
    collections::hash_map::RandomState,
    hash::BuildHasher,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use mixrelay_control_protocol::ControlStage;
use mixrelay_desktop_core::{
    engine_control::receipt::{
        self, ENGINE_HEADER, EXPECTED_ENGINE_HEADER, Next, RECEIPT_HEADER, REQUEST_HEADER,
        RESULT_HEADER, ResponseHeaders,
    },
    engine_control::{rejection::Rejection, routes},
    timing::AsyncDelay,
};
use mixrelay_time::Deadline;
use reqwest::{Method, Response, header::HeaderMap};
use serde_json::Value;

use super::{ControlProxyClient, ControlProxyResponse};
use transport_failure::TransportFailure;

mod transport_failure;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(5);
const ACKNOWLEDGEMENT_TIMEOUT: Duration = Duration::from_secs(1);
const RECEIPT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// One request's exchange with the engine at `origin`; `timer` paces receipt polls.
pub(super) struct Exchange<'a, T> {
    client: &'a ControlProxyClient,
    origin: &'a str,
    timer: &'a T,
}

enum ExchangeError {
    /// The transport failed; the engine may or may not have the request.
    Transport(TransportFailure),
    /// The engine answered with something that cannot be this request's outcome.
    Invalid(String),
}

impl<'a, T: AsyncDelay + Sync> Exchange<'a, T> {
    pub(super) fn new(client: &'a ControlProxyClient, origin: &'a str, timer: &'a T) -> Self {
        Self {
            client,
            origin,
            timer,
        }
    }

    pub(super) async fn request(
        &self,
        method: Method,
        path: &str,
        mut body: Option<Value>,
    ) -> Result<ControlProxyResponse, String> {
        let supplied = receipt::take_client_operation_id(body.as_mut(), path);
        if receipt::is_receipt_observation(method.as_str(), path) {
            return self.observe_receipt(path).await;
        }
        let requested = receipt::requires_client_operation(method.as_str(), path)
            .then(|| supplied.unwrap_or_else(new_operation_id));
        let sent = match self.send(method, path, body, requested.as_deref()).await {
            Ok(response) => self.finish(response, requested.as_deref(), None).await,
            // Nothing reached the engine, so there is no receipt to resolve.
            Err(error) if error.is_connect() => {
                return Ok(failed(path, &TransportFailure::request(error), false));
            }
            Err(error) => Err(ExchangeError::Transport(TransportFailure::request(error))),
        };
        match (sent, requested) {
            (Ok(response), _) => Ok(response),
            (Err(ExchangeError::Invalid(reason)), _) => Err(reason),
            (Err(ExchangeError::Transport(error)), None) => Ok(failed(path, &error, true)),
            (Err(ExchangeError::Transport(error)), Some(id)) => {
                self.recover(path, &id, &error).await
            }
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        requested: Option<&str>,
    ) -> Result<Response, reqwest::Error> {
        let mut request = self
            .client
            .client
            .request(method, format!("{}{path}", self.origin))
            .timeout(REQUEST_TIMEOUT);
        if let Some(id) = requested {
            request = request.header(REQUEST_HEADER, id);
            if let Some(engine) = expected_engine(self.client) {
                request = request.header(EXPECTED_ENGINE_HEADER, engine);
            }
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        request.send().await
    }

    /// Headers and even part of a body can arrive before transport fails. Resolves the
    /// outcome once by ID, including a failure while polling; never resubmits the work.
    async fn recover(
        &self,
        path: &str,
        id: &str,
        error: &TransportFailure,
    ) -> Result<ControlProxyResponse, String> {
        let Ok(route) = routes::operation(id) else {
            return Ok(failed(path, error, true));
        };
        let Ok(response) = self.get(&route, RECEIPT_TIMEOUT).await else {
            return Ok(failed(path, error, true));
        };
        let engine = observe_engine(self.client, response.headers());
        if !response_headers(&response).concerns(id) {
            let mut uncertain = failed(path, error, true);
            uncertain.engine = engine;
            return Ok(uncertain);
        }
        match self.finish(response, Some(id), Some(id.to_owned())).await {
            Ok(response) => Ok(response),
            Err(ExchangeError::Invalid(reason)) => Err(reason),
            Err(ExchangeError::Transport(error)) => Ok(failed(path, &error, true)),
        }
    }

    /// Follows a response to its outcome, polling a pending receipt until it ends.
    async fn finish(
        &self,
        mut response: Response,
        requested: Option<&str>,
        mut accepted: Option<String>,
    ) -> Result<ControlProxyResponse, ExchangeError> {
        let mut stage = if accepted.is_some() {
            ControlStage::ReceiptObservation
        } else {
            ControlStage::Response
        };
        loop {
            let engine = observe_engine(self.client, response.headers());
            let next = response_headers(&response)
                .next(requested, accepted.as_deref())
                .map_err(|error| ExchangeError::Invalid(error.to_string()))?;
            let status = response.status().as_u16();
            let body = read_body(response, stage).await?;
            let id = match next {
                Next::Complete { acknowledge } => {
                    log_rejection(status, &body);
                    if let Some(id) = acknowledge {
                        self.acknowledge(&id).await;
                    }
                    return Ok(ControlProxyResponse::answered(engine, status, body));
                }
                Next::Poll(id) => id,
            };
            receipt::confirm_receipt_body(&id, &body)
                .map_err(|error| ExchangeError::Invalid(error.to_string()))?;
            self.client.operation_liveness.notify_waiters();
            self.timer
                .wait_until(Deadline::after(self.timer.now(), RECEIPT_POLL_INTERVAL))
                .await;
            stage = ControlStage::ReceiptObservation;
            let route = routes::operation(&id)
                .map_err(|error| ExchangeError::Invalid(error.to_string()))?;
            response = self.get(&route, RECEIPT_TIMEOUT).await.map_err(|error| {
                ExchangeError::Transport(TransportFailure::at(
                    ControlStage::ReceiptObservation,
                    error,
                ))
            })?;
            accepted = Some(id);
        }
    }

    /// Read-only receipt observation does not consume or acknowledge the writer's result.
    async fn observe_receipt(&self, path: &str) -> Result<ControlProxyResponse, String> {
        let response = self
            .get(path, RECEIPT_TIMEOUT)
            .await
            .map_err(|error| error.to_string())?;
        let engine = observe_engine(self.client, response.headers());
        let status = response.status().as_u16();
        let body = response.json().await.map_err(|error| error.to_string())?;
        Ok(ControlProxyResponse::answered(engine, status, body))
    }

    /// Lets the engine reclaim a stored result. A lost acknowledgement only delays that; it
    /// must never replace an already observed result with an error.
    async fn acknowledge(&self, id: &str) {
        let Ok(route) = routes::operation(id) else {
            return;
        };
        let url = format!("{}{route}", self.origin);
        match self
            .client
            .client
            .delete(url)
            .timeout(ACKNOWLEDGEMENT_TIMEOUT)
            .send()
            .await
        {
            Ok(response) => {
                observe_engine(self.client, response.headers());
            }
            Err(error) => tracing::debug!(?error, "control receipt acknowledgement failed"),
        }
    }

    async fn get(&self, path: &str, timeout: Duration) -> Result<Response, reqwest::Error> {
        self.client
            .client
            .get(format!("{}{path}", self.origin))
            .timeout(timeout)
            .send()
            .await
    }
}

/// Reads a whole body. Byte reads and JSON parsing stay separate because reqwest reports
/// some body transport errors as decode errors too.
async fn read_body(response: Response, stage: ControlStage) -> Result<Value, ExchangeError> {
    if response.status() == 204 {
        return Ok(Value::Null);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|source| ExchangeError::Transport(TransportFailure::at(stage, source)))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        ExchangeError::Invalid(format!("local control response was invalid: {error}"))
    })
}

fn response_headers(response: &Response) -> ResponseHeaders<'_> {
    let header = |name| response.headers().get(name);
    ResponseHeaders {
        status: response.status().as_u16(),
        // An unreadable receipt is invalid rather than absent.
        receipt: header(RECEIPT_HEADER).map(|value| value.to_str().unwrap_or_default()),
        result: header(RESULT_HEADER).and_then(|value| value.to_str().ok()),
    }
}

/// Records the engine incarnation that answered, which later mutations are fenced to.
pub(super) fn observe_engine(client: &ControlProxyClient, headers: &HeaderMap) -> Option<String> {
    let engine = headers
        .get(ENGINE_HEADER)
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

fn expected_engine(client: &ControlProxyClient) -> Option<String> {
    client
        .engine
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
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

/// Server rejections are logged apart from failures to reach the server.
fn log_rejection(status: u16, body: &Value) {
    if let Some(rejection) = Rejection::from_response(status, body) {
        tracing::warn!(
            platform = ?crate::control_channel::failure_platform(),
            boundary = "runtime_engine",
            stage = "server_response",
            status,
            code = rejection.code,
            outcome = rejection.outcome,
            "engine rejected control request"
        );
    }
}

/// The editor's answer to an exchange whose transport failed.
fn failed(path: &str, error: &TransportFailure, accepted: bool) -> ControlProxyResponse {
    let failure = error.report(path, accepted);
    let diagnostic = failure.diagnostic();
    tracing::warn!(path, platform = ?diagnostic.platform, boundary = ?diagnostic.boundary,
        stage = ?diagnostic.stage, failure = ?diagnostic.failure, delivery = ?diagnostic.delivery,
        os_error = diagnostic.os_error, "control exchange failed");
    ControlProxyResponse::delivered(None, 503, failure.body(), failure.delivery())
}

#[cfg(test)]
mod tests;
