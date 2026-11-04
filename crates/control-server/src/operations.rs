use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Extension, Json,
    body::{Body, Bytes, to_bytes},
    extract::{Path, Request, State as AxumState},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header::CONTENT_TYPE},
    middleware::Next,
    response::{IntoResponse, Response},
};
use mixrelay_control_protocol::{ErrorCode, ProtocolError};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

mod ledger;
use ledger::{Ack, ReceiptLedger, Rejection, Snapshot};
use mixrelay_time::MonotonicClock;

pub(super) const MAX_RECEIPTS: usize = 8;
pub(super) const RESPONSE_LIMIT: usize = 16 * 1024 * 1024;
const INGRESS_PERMITS: usize = 8;
const COMPLETED_TTL: Duration = Duration::from_mins(1);
const OPERATION_HEADER: &str = "x-mixrelay-operation";
const REQUEST_HEADER: &str = "x-mixrelay-operation-request";
const ENGINE_HEADER: &str = "x-mixrelay-engine";
const RESULT_HEADER: &str = "x-mixrelay-operation-result";

#[derive(Clone, Copy)]
struct UncertainTask;

/// Marks a lost blocking task as uncertain so polling cannot confirm its mutation outcome.
pub(super) fn task_failure(code: ErrorCode, reason: &'static str) -> Response {
    let mut response = (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ProtocolError {
            code,
            parameters: serde_json::json!({"operationOutcome": "uncertain", "reason": reason}),
        }),
    )
        .into_response();
    response.extensions_mut().insert(UncertainTask);
    response
}

/// Owns bounded long-running control operations for one server lifetime.
#[derive(Clone)]
pub struct ControlOperations {
    inner: Arc<Inner>,
    timings: OperationTimings,
}

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

struct Inner {
    ledger: Mutex<ReceiptLedger<Box<StoredResponse>, Arc<Notify>>>,
    clock: Box<dyn MonotonicClock>,
    ingress: Arc<Semaphore>,
    owner_nonce: u128,
    next_id: std::sync::atomic::AtomicU64,
    wake: Notify,
    /// Becomes true when admission closes; long-lived streams end with it.
    closing: tokio::sync::watch::Sender<bool>,
}

#[derive(Clone, Debug)]
struct StoredResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
    original: bool,
}

#[derive(serde::Serialize)]
struct PendingResponse<'a> {
    #[serde(rename = "operationId")]
    operation_id: &'a str,
}

impl ControlOperations {
    /// Creates an open operation owner with a fresh session identifier namespace.
    /// Receipt expiry uses the supplied monotonic clock.
    #[must_use]
    pub fn new(clock: Box<dyn MonotonicClock>) -> Self {
        static OWNER_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let sequence = OWNER_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            inner: Arc::new(Inner {
                ledger: Mutex::new(ReceiptLedger::new(MAX_RECEIPTS, COMPLETED_TTL)),
                clock,
                ingress: Arc::new(Semaphore::new(INGRESS_PERMITS)),
                owner_nonce: now ^ (u128::from(sequence) << 64),
                next_id: std::sync::atomic::AtomicU64::new(1),
                wake: Notify::new(),
                closing: tokio::sync::watch::Sender::new(false),
            }),
            timings: OperationTimings::default(),
        }
    }

    /// Replaces the time bounds, for example short ones in a test harness.
    #[must_use]
    pub fn with_timings(mut self, timings: OperationTimings) -> Self {
        self.timings = timings;
        self
    }

    pub(crate) fn timings(&self) -> OperationTimings {
        self.timings
    }

    /// Stops accepting new work while allowing all admitted handlers to finish.
    pub fn close_admission(&self) {
        self.lock_ledger().close();
        self.inner.closing.send_replace(true);
    }

    /// Resolves once admission closes, so streamed responses let graceful shutdown finish.
    pub(crate) fn closed(&self) -> impl std::future::Future<Output = ()> + Send + use<> {
        let mut closing = self.inner.closing.subscribe();
        async move {
            let _ = closing.wait_for(|closing| *closing).await;
        }
    }

    /// Waits until every admitted handler has published a terminal receipt.
    pub async fn drain(&self) {
        loop {
            let notified = self.inner.wake.notified();
            if self.lock_ledger().pending() == 0 {
                return;
            }
            notified.await;
        }
    }

    pub(crate) fn try_acquire_ingress(&self) -> Option<IngressLease> {
        self.inner
            .ingress
            .clone()
            .try_acquire_owned()
            .ok()
            .map(|permit| IngressLease {
                _permit: Arc::new(permit),
            })
    }

    fn now(&self) -> mixrelay_time::Instant {
        self.inner.clock.now()
    }

    fn admit<'a>(
        &self,
        requested: Option<&'a str>,
    ) -> Result<Admission, Rejection<'a, Box<StoredResponse>>> {
        let notify = Arc::new(Notify::new());
        let now = self.now();
        let id = self
            .lock_ledger()
            .admit(requested, notify.clone(), now, || {
                let sequence = self
                    .inner
                    .next_id
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                format!("op-{:032x}-{:016x}", self.inner.owner_nonce, sequence)
            })?;
        Ok(Admission { id, notify })
    }

    fn complete(&self, id: &str, response: StoredResponse) {
        let now = self.now();
        let Some(notify) = self.lock_ledger().complete(id, Box::new(response), now) else {
            return;
        };
        notify.notify_waiters();
        self.inner.wake.notify_waiters();
    }

    fn snapshot(&self, id: &str) -> Snapshot<Box<StoredResponse>> {
        let now = self.now();
        self.lock_ledger().snapshot(id, now)
    }

    fn acknowledge(&self, id: &str) -> Ack {
        let now = self.now();
        self.lock_ledger().acknowledge(id, now)
    }

    fn take_completed(&self, id: &str) -> Option<StoredResponse> {
        self.lock_ledger()
            .take_completed(id)
            .map(|response| *response)
    }

    fn lock_ledger(&self) -> MutexGuard<'_, ReceiptLedger<Box<StoredResponse>, Arc<Notify>>> {
        self.inner
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct Admission {
    id: String,
    notify: Arc<Notify>,
}

#[derive(Clone)]
pub(crate) struct IngressLease {
    _permit: Arc<OwnedSemaphorePermit>,
}

fn valid_client_id(id: &str) -> bool {
    (16..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn client_operation_id(request: &Request) -> Result<Option<&str>, ErrorCode> {
    match request.headers().get(REQUEST_HEADER) {
        None => Ok(None),
        Some(header) => match header.to_str() {
            Ok(id) if valid_client_id(id) => Ok(Some(id)),
            _ => Err(ErrorCode::InvalidOperationId),
        },
    }
}

/// Resolve retained identities before touching the body or its admission budget.
/// Dispatch still checks atomically when admitting a new identity after ingress.
pub(super) fn before_ingress(
    operations: &ControlOperations,
    request: &Request,
) -> Option<Response> {
    if !needs_operation(request) {
        return None;
    }
    let id = match client_operation_id(request) {
        Ok(id) => id?,
        Err(code) => return Some(not_admitted(StatusCode::BAD_REQUEST, code)),
    };
    match operations.snapshot(id) {
        Snapshot::Pending => Some(pending_response(id)),
        Snapshot::Complete(response) => Some(response.into_response(Some(id))),
        Snapshot::Missing => None,
    }
}

fn not_admitted(status: StatusCode, code: ErrorCode) -> Response {
    (
        status,
        Json(ProtocolError {
            code,
            parameters: serde_json::json!({"operationOutcome": "not_admitted"}),
        }),
    )
        .into_response()
}

/// Converts a backend incarnation into the response header once, at router construction.
/// An empty incarnation means the backend has no identity; an incarnation that cannot be a
/// header value is reported and treated the same, so clients never receive a fence for it.
pub(super) fn engine_header(incarnation: String) -> Option<HeaderValue> {
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
pub(super) async fn engine_identity(
    AxumState(engine): AxumState<Option<HeaderValue>>,
    request: Request,
    next: Next,
) -> Response {
    let incarnation = engine.as_ref().map_or(&b""[..], HeaderValue::as_bytes);
    let mismatch = needs_operation(&request)
        && request
            .headers()
            .get("x-mixrelay-expected-engine")
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

pub(super) async fn dispatch(
    Extension(operations): Extension<ControlOperations>,
    mut request: Request,
    next: Next,
) -> Response {
    if !needs_operation(&request) {
        let lease = request.extensions_mut().remove::<IngressLease>();
        let response = next.run(request).await;
        drop(lease);
        return response;
    }
    let lease = request.extensions_mut().remove::<IngressLease>();
    let requested = match client_operation_id(&request) {
        Ok(id) => id.map(str::to_owned),
        Err(code) => return not_admitted(StatusCode::BAD_REQUEST, code),
    };
    let admission = match operations.admit(requested.as_deref()) {
        Ok(admission) => admission,
        Err(Rejection::Pending(id)) => return pending_response(id),
        Err(Rejection::Complete(id, response)) => return response.into_response(Some(id)),
        Err(Rejection::Full) => return admission_full_response(&request),
    };
    let operation_id = admission.id.clone();
    let task_operations = operations.clone();
    tokio::spawn(async move {
        let _lease = lease;
        let response = match tokio::spawn(next.run(request)).await {
            Ok(response) => collect_response(response).await,
            Err(error) => uncertain_response(&format!("operation task failed: {error}")),
        };
        task_operations.complete(&operation_id, response);
    });

    let completed = completion_after_wait(&operations, &admission.id, admission.notify).await;
    if completed && requested.is_some() {
        return get(Extension(operations), Path(admission.id)).await;
    }
    if completed && let Some(response) = operations.take_completed(&admission.id) {
        return response.into_response(None);
    }
    pending_response(&admission.id)
}

async fn completion_after_wait(
    operations: &ControlOperations,
    id: &str,
    notify: Arc<Notify>,
) -> bool {
    let notified = notify.notified();
    if matches!(operations.snapshot(id), Snapshot::Complete(_)) {
        return true;
    }
    let _ = tokio::time::timeout(operations.timings().completion_wait, notified).await;
    matches!(operations.snapshot(id), Snapshot::Complete(_))
}

pub(super) async fn get(
    Extension(operations): Extension<ControlOperations>,
    Path(id): Path<String>,
) -> Response {
    match operations.snapshot(&id) {
        Snapshot::Pending => pending_response(&id),
        Snapshot::Complete(response) => response.into_response(Some(&id)),
        Snapshot::Missing => missing_response(),
    }
}

pub(super) async fn delete(
    Extension(operations): Extension<ControlOperations>,
    Path(id): Path<String>,
) -> Response {
    match operations.acknowledge(&id) {
        Ack::Completed => StatusCode::NO_CONTENT.into_response(),
        Ack::Pending => {
            let mut response = (
                StatusCode::CONFLICT,
                Json(ProtocolError {
                    code: ErrorCode::ControlUnavailable,
                    parameters: serde_json::json!({"operationOutcome": "pending"}),
                }),
            )
                .into_response();
            add_operation_header(response.headers_mut(), &id);
            response
        }
        Ack::Missing => missing_response(),
    }
}

async fn collect_response(response: Response) -> StoredResponse {
    let (parts, body) = response.into_parts();
    let mut headers = parts.headers;
    headers.remove(HeaderName::from_static(OPERATION_HEADER));
    headers.remove(HeaderName::from_static(RESULT_HEADER));
    match to_bytes(body, RESPONSE_LIMIT.saturating_add(1)).await {
        Ok(body) if body.len() <= RESPONSE_LIMIT => StoredResponse {
            status: parts.status,
            headers,
            body,
            original: parts.extensions.get::<UncertainTask>().is_none(),
        },
        Ok(_) => uncertain_response("operation response exceeded bounded storage"),
        Err(error) => uncertain_response(&format!("operation response failed: {error}")),
    }
}

fn uncertain_response(reason: &str) -> StoredResponse {
    let error = ProtocolError {
        code: ErrorCode::ControlUnavailable,
        parameters: serde_json::json!({
            "operationOutcome": "uncertain",
            "reason": reason,
        }),
    };
    let body = serde_json::to_vec(&error)
        .unwrap_or_else(|_| b"{\"code\":\"control_unavailable\"}".to_vec());
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    StoredResponse {
        status: StatusCode::SERVICE_UNAVAILABLE,
        headers,
        body: Bytes::from(body),
        original: false,
    }
}

fn pending_response(id: &str) -> Response {
    let mut response = Json(PendingResponse { operation_id: id }).into_response();
    *response.status_mut() = StatusCode::ACCEPTED;
    add_operation_header(response.headers_mut(), id);
    response
}

pub(crate) fn admission_full_response(request: &Request) -> Response {
    let mut parameters = serde_json::json!({});
    if request.method() != Method::GET {
        let outcome = match request.uri().path() {
            crate::GRAPH_CATALOG_ROUTE => "catalogOutcome",
            "/api/v1/workspace" | "/api/v1/workspace/preview" => "workspaceOutcome",
            "/api/v1/audio/power" | "/api/v1/audio/power/preview" => "powerOutcome",
            path if path.starts_with("/api/v1/graphs/") && path.contains("/nodes/") => {
                "nodeControlOutcome"
            }
            _ => "operationOutcome",
        };
        parameters[outcome] = serde_json::json!("not_applied");
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters,
        }),
    )
        .into_response()
}

fn missing_response() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({"operationOutcome": "uncertain"}),
        }),
    )
        .into_response()
}

fn add_operation_header(headers: &mut HeaderMap, id: &str) {
    if let Ok(value) = HeaderValue::try_from(id) {
        headers.insert(HeaderName::from_static(OPERATION_HEADER), value);
    }
}

impl StoredResponse {
    fn into_response(self, receipt_id: Option<&str>) -> Response {
        let mut response = Response::new(Body::from(self.body));
        *response.status_mut() = self.status;
        *response.headers_mut() = self.headers;
        if let Some(receipt_id) = receipt_id.filter(|_| self.original)
            && let Ok(value) = HeaderValue::try_from(receipt_id)
        {
            response
                .headers_mut()
                .insert(HeaderName::from_static(RESULT_HEADER), value);
        }
        response
    }
}

fn needs_operation(request: &Request) -> bool {
    let path = request.uri().path();
    if path.starts_with("/api/v1/operations/") || path == "/api/v1/operations" {
        return false;
    }
    if path == crate::HEALTH_ROUTE
        || path == crate::STATUS_ROUTE
        || path == crate::GRAPH_STATUS_ROUTE
        || path == crate::CONTROL_SNAPSHOT_ROUTE
        || path == crate::METERS_ROUTE
        || path.ends_with("/editor/frame")
        || path.ends_with("/editor/input")
    {
        return false;
    }
    if request.method() == Method::GET {
        return path == crate::GRAPH_ROUTE
            || path == crate::GRAPH_CATALOG_ROUTE
            || path.starts_with("/api/v1/graphs/")
            || path == crate::PROCESSORS_ROUTE
            || path == crate::APPLICATION_CAPTURE_MANIFEST_ROUTE;
    }
    request.method() == Method::POST || request.method() == Method::PUT
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Extension, Json, Router,
        body::{Body, to_bytes},
        http::Request,
        middleware,
        routing::{MethodRouter, get as route_get, post},
    };
    use mixrelay_time::ManualClock;
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    /// Long enough for an immediate handler to complete inside the window on a loaded runner.
    const COMPLETION_WAIT: Duration = Duration::from_millis(25);
    /// A handler that outlasts the completion window and becomes a pending operation.
    const SLOW_HANDLER: Duration = Duration::from_millis(60);

    fn fast_operations() -> ControlOperations {
        ControlOperations::new(crate::tests::clock()).with_timings(OperationTimings {
            completion_wait: COMPLETION_WAIT,
            ..OperationTimings::default()
        })
    }

    fn test_router(operations: ControlOperations, work: MethodRouter) -> Router {
        Router::new()
            .route("/work", work)
            .route(
                "/api/v1/operations/{id}",
                route_get(super::get).delete(super::delete),
            )
            .layer(middleware::from_fn(super::dispatch))
            .layer(Extension(operations))
    }

    async fn pending_id(response: axum::response::Response) -> String {
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let header = response
            .headers()
            .get(OPERATION_HEADER)
            .expect("pending operation header")
            .to_str()
            .expect("operation id header")
            .to_owned();
        let body = to_bytes(response.into_body(), 4096)
            .await
            .expect("pending response body");
        let body: Value = serde_json::from_slice(&body).expect("pending JSON");
        assert_eq!(body["operationId"], header);
        header
    }

    async fn terminal_response(
        operations: &ControlOperations,
        id: &str,
    ) -> axum::response::Response {
        for _ in 0..200 {
            if matches!(operations.snapshot(id), Snapshot::Complete(_)) {
                return super::get(Extension(operations.clone()), Path(id.to_owned())).await;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("operation did not complete");
    }

    fn stored(status: StatusCode, body: &'static [u8], original: bool) -> StoredResponse {
        StoredResponse {
            status,
            headers: HeaderMap::new(),
            body: Bytes::from_static(body),
            original,
        }
    }

    fn panic_status() -> StatusCode {
        panic!("handler panic");
    }

    #[tokio::test]
    async fn client_ids_retain_fast_results_and_deduplicate_even_when_admission_is_closed() {
        let operations = fast_operations();
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();
        let router = test_router(
            operations.clone(),
            post(move || {
                handler_calls.fetch_add(1, Ordering::SeqCst);
                async {
                    (
                        StatusCode::CONFLICT,
                        Json(json!({"code":"graph_exchange_busy"})),
                    )
                }
            }),
        );
        let id = "Client_0123456789-";
        for _ in 0..2 {
            let response = router
                .clone()
                .oneshot(
                    Request::post("/work")
                        .header(REQUEST_HEADER, id)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT);
            assert_eq!(response.headers()[RESULT_HEADER], id);
            let body = to_bytes(response.into_body(), 4096).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap()["code"],
                "graph_exchange_busy"
            );
            operations.close_admission();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(operations.lock_ledger().len(), 1);
        assert_eq!(
            super::delete(Extension(operations.clone()), Path(id.into()))
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
        assert!(matches!(operations.snapshot(id), Snapshot::Missing));
    }

    #[tokio::test]
    async fn duplicate_pending_id_does_not_execute_or_consume_another_slot() {
        let operations = fast_operations();
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();
        let handler_release = release.clone();
        let router = test_router(
            operations.clone(),
            post(move || {
                handler_calls.fetch_add(1, Ordering::SeqCst);
                let release = handler_release.clone();
                async move {
                    release.notified().await;
                    Json(json!({"ok":true}))
                }
            }),
        );
        let id = "pending-client-01";
        let first = router
            .clone()
            .oneshot(
                Request::post("/work")
                    .header(REQUEST_HEADER, id)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(pending_id(first).await, id);
        for _ in 1..MAX_RECEIPTS {
            operations.admit(None).unwrap();
        }
        let duplicate = router
            .oneshot(
                Request::post("/work")
                    .header(REQUEST_HEADER, id)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(pending_id(duplicate).await, id);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(operations.lock_ledger().len(), MAX_RECEIPTS);
        release.notify_one();
        assert_eq!(
            terminal_response(&operations, id).await.status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn existing_ids_resolve_before_body_reads_with_saturated_full_middleware() {
        let operations = fast_operations();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_release = release.clone();
        let handler_calls = calls.clone();
        let handlers = Router::new().route(
            "/work",
            post(move |body: Bytes| {
                let release = handler_release.clone();
                let calls = handler_calls.clone();
                async move {
                    assert_eq!(body, "{\"work\":true}");
                    calls.fetch_add(1, Ordering::SeqCst);
                    release.acquire().await.unwrap().forget();
                    (
                        StatusCode::NOT_FOUND,
                        Json(json!({"code":"node_not_found"})),
                    )
                }
            }),
        );
        let router = crate::operation_middleware(handlers, operations.clone(), "engine".into());
        saturate_ingress(&router).await;
        assert!(operations.try_acquire_ingress().is_none());
        assert_eq!(calls.load(Ordering::SeqCst), MAX_RECEIPTS);
        // A body that never completes would time out if ingress tried to read it.
        let unread_request = |id: &str| {
            Request::post("/work")
                .header(REQUEST_HEADER, id)
                .body(Body::from_stream(futures_util::stream::pending::<
                    Result<Bytes, std::io::Error>,
                >()))
                .unwrap()
        };
        let duplicate = router
            .clone()
            .oneshot(unread_request("client-operation-0"))
            .await
            .unwrap();
        assert_eq!(duplicate.headers()[ENGINE_HEADER], "engine");
        assert_eq!(duplicate.headers()[axum::http::header::CONNECTION], "close");
        assert_eq!(pending_id(duplicate).await, "client-operation-0");
        let new_id = router
            .clone()
            .oneshot(
                Request::post("/work")
                    .header(REQUEST_HEADER, "client-operation-new")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(new_id.status(), StatusCode::SERVICE_UNAVAILABLE);
        let invalid = router.clone().oneshot(unread_request("bad")).await.unwrap();
        assert_invalid_id(invalid).await;
        release.add_permits(MAX_RECEIPTS);
        operations.drain().await;
        // Retained terminal errors must also bypass ingress, even while closing.
        let _held: Vec<_> = (0..INGRESS_PERMITS)
            .map(|_| operations.try_acquire_ingress().unwrap())
            .collect();
        operations.close_admission();
        let completed = router
            .oneshot(unread_request("client-operation-0"))
            .await
            .unwrap();
        assert_eq!(completed.status(), StatusCode::NOT_FOUND);
        assert_eq!(completed.headers()[RESULT_HEADER], "client-operation-0");
        assert_eq!(completed.headers()[ENGINE_HEADER], "engine");
        let body = to_bytes(completed.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["code"],
            "node_not_found"
        );
        assert_eq!(calls.load(Ordering::SeqCst), MAX_RECEIPTS);
    }

    async fn saturate_ingress(router: &Router) {
        for index in 0..MAX_RECEIPTS {
            let response = router
                .clone()
                .oneshot(
                    Request::post("/work")
                        .header(REQUEST_HEADER, format!("client-operation-{index}"))
                        .body(Body::from(r#"{"work":true}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::ACCEPTED);
        }
    }

    async fn assert_invalid_id(response: Response) {
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let error: ProtocolError = serde_json::from_slice(&body).unwrap();
        assert_eq!(error.code, ErrorCode::InvalidOperationId);
        assert_eq!(error.parameters["operationOutcome"], "not_admitted");
    }

    #[tokio::test]
    async fn validates_client_ids_before_admission() {
        let operations = fast_operations();
        let router = test_router(
            operations.clone(),
            post(|| async { StatusCode::NO_CONTENT }),
        );
        for id in [
            String::new(),
            "a".repeat(15),
            "a".repeat(65),
            "a".repeat(15) + "/",
            "a".repeat(15) + ".",
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::post("/work")
                        .header(REQUEST_HEADER, id)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_invalid_id(response).await;
        }
        let response = router
            .clone()
            .oneshot(
                Request::post("/work")
                    .header(REQUEST_HEADER, HeaderValue::from_bytes(b"\xff").unwrap())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_invalid_id(response).await;
        assert_eq!(operations.lock_ledger().len(), 0);
        for id in ["a".repeat(16), "Z0_-".repeat(16)] {
            let response = router
                .clone()
                .oneshot(
                    Request::post("/work")
                        .header(REQUEST_HEADER, &id)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
            assert_eq!(response.headers()[RESULT_HEADER], id);
        }
    }

    #[test]
    fn generated_ids_cannot_overwrite_client_records_and_ack_allows_reuse() {
        let operations = fast_operations();
        let id = format!("op-{:032x}-{:016x}", operations.inner.owner_nonce, 1);
        let client = operations.admit(Some(&id)).unwrap();
        let generated = operations.admit(None).unwrap();
        assert_ne!(client.id, generated.id);
        assert_eq!(operations.lock_ledger().len(), 2);
        operations.complete(&client.id, stored(StatusCode::OK, b"ok", true));
        assert!(matches!(operations.acknowledge(&client.id), Ack::Completed));
        assert_eq!(operations.admit(Some(&id)).unwrap().id, id);
    }

    #[tokio::test]
    async fn fence_precedes_admission_and_does_not_create_a_receipt() {
        let operations = fast_operations();
        let router = test_router(
            operations.clone(),
            post(|| async { StatusCode::NO_CONTENT }),
        )
        .layer(middleware::from_fn_with_state(
            Some(HeaderValue::from_static("new-engine")),
            engine_identity,
        ));
        operations.close_admission();
        let response = router
            .oneshot(
                Request::post("/work")
                    .header("x-mixrelay-expected-engine", "old-engine")
                    .header(REQUEST_HEADER, "client-operation-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(response.headers()[ENGINE_HEADER], "new-engine");
        assert_eq!(operations.lock_ledger().len(), 0);
    }

    #[tokio::test]
    async fn client_receipts_are_bounded_until_acknowledged_or_expired() {
        let clock = ManualClock::new(mixrelay_time::Instant::from_origin(Duration::ZERO));
        let operations = ControlOperations::new(Box::new(clock.clone()));
        let router = test_router(
            operations.clone(),
            post(|| async { StatusCode::NO_CONTENT }),
        );
        let first = "client-operation-0";
        for sequence in 0..=MAX_RECEIPTS {
            let response = router
                .clone()
                .oneshot(
                    Request::post("/work")
                        .header(REQUEST_HEADER, format!("client-operation-{sequence}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if sequence < MAX_RECEIPTS {
                    StatusCode::NO_CONTENT
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                }
            );
        }
        let duplicate = router
            .clone()
            .oneshot(
                Request::post("/work")
                    .header(REQUEST_HEADER, first)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(duplicate.status(), StatusCode::NO_CONTENT);
        assert_eq!(duplicate.headers()[RESULT_HEADER], first);
        assert!(matches!(operations.acknowledge(first), Ack::Completed));
        operations.admit(Some(first)).unwrap();
        operations.complete(first, stored(StatusCode::OK, b"ok", true));
        clock.advance(COMPLETED_TTL);
        assert_eq!(operations.admit(Some(first)).unwrap().id, first);
    }

    #[test]
    fn ids_change_between_owners_and_requests() {
        let first = fast_operations();
        let second = fast_operations();
        let first_id = first.admit(None).expect("first admission").id;
        let second_id = second.admit(None).expect("second admission").id;
        assert_ne!(first_id, second_id);
    }

    #[test]
    fn admission_is_bounded_without_eviction_of_pending_work() {
        let operations = fast_operations();
        for _ in 0..MAX_RECEIPTS {
            operations.admit(None).expect("bounded admission");
        }
        assert!(operations.admit(None).is_err());
    }

    #[tokio::test]
    async fn slow_success_is_polled_once_and_acknowledged() {
        let operations = fast_operations();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let handler_started = started.clone();
        let handler_release = release.clone();
        let router = test_router(
            operations.clone(),
            post(move || async move {
                handler_started.notify_one();
                handler_release.notified().await;
                Json(json!({"ok": true}))
            }),
        );
        let request = tokio::spawn(
            router.oneshot(Request::post("/work").body(Body::empty()).expect("request")),
        );
        started.notified().await;
        let id = pending_id(request.await.expect("request task").expect("response")).await;
        release.notify_one();
        let response = terminal_response(&operations, &id).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get(RESULT_HEADER).unwrap(), id.as_str());
        let acknowledged = super::delete(Extension(operations.clone()), Path(id.clone())).await;
        assert_eq!(acknowledged.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            super::get(Extension(operations), Path(id)).await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn slow_original_error_keeps_result_identity() {
        let operations = fast_operations();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let handler_started = started.clone();
        let handler_release = release.clone();
        let router = test_router(
            operations.clone(),
            post(move || async move {
                handler_started.notify_one();
                handler_release.notified().await;
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({"code": "graph_invalid"})),
                )
            }),
        );
        let request = tokio::spawn(
            router.oneshot(Request::post("/work").body(Body::empty()).expect("request")),
        );
        started.notified().await;
        let id = pending_id(request.await.expect("request task").expect("response")).await;
        release.notify_one();
        let response = terminal_response(&operations, &id).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(response.headers().get(RESULT_HEADER).unwrap(), id.as_str());
    }

    #[tokio::test]
    async fn original_accepted_response_has_no_operation_header() {
        let operations = fast_operations();
        let router = test_router(
            operations,
            post(|| async { (StatusCode::ACCEPTED, Json(json!({"accepted": true}))) }),
        );
        let response = router
            .oneshot(Request::post("/work").body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(response.headers().get(OPERATION_HEADER).is_none());
        assert!(response.headers().get(RESULT_HEADER).is_none());
    }

    #[tokio::test]
    async fn disconnected_waiter_does_not_replay_work() {
        let operations = fast_operations();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_started = started.clone();
        let handler_release = release.clone();
        let handler_calls = calls.clone();
        let router = test_router(
            operations.clone(),
            post(move || async move {
                handler_calls.fetch_add(1, Ordering::SeqCst);
                handler_started.notify_one();
                handler_release.notified().await;
                StatusCode::NO_CONTENT
            }),
        );
        let request = tokio::spawn(
            router.oneshot(Request::post("/work").body(Body::empty()).expect("request")),
        );
        started.notified().await;
        request.abort();
        release.notify_one();
        for _ in 0..100 {
            if operations.lock_ledger().pending() == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(operations.lock_ledger().pending(), 0);
    }

    #[tokio::test]
    async fn quick_responses_reclaim_receipts() {
        let operations = fast_operations();
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();
        let router = test_router(
            operations,
            post(move || {
                handler_calls.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::NO_CONTENT }
            }),
        );
        for _ in 0..(MAX_RECEIPTS * 2) {
            let response = router
                .clone()
                .oneshot(Request::post("/work").body(Body::empty()).expect("request"))
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
        }
        assert_eq!(calls.load(Ordering::SeqCst), MAX_RECEIPTS * 2);
    }

    #[tokio::test]
    async fn middleware_rejects_the_ninth_pending_request_before_handler() {
        let operations = fast_operations();
        let started = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let handler_started = started.clone();
        let handler_release = release.clone();
        let router = test_router(
            operations,
            post(move || {
                handler_started.fetch_add(1, Ordering::SeqCst);
                let handler_release = handler_release.clone();
                async move {
                    handler_release.notified().await;
                    StatusCode::NO_CONTENT
                }
            }),
        );
        let requests: Vec<_> = (0..MAX_RECEIPTS)
            .map(|_| {
                let router = router.clone();
                tokio::spawn(async move {
                    router
                        .oneshot(Request::post("/work").body(Body::empty()).expect("request"))
                        .await
                        .expect("response")
                })
            })
            .collect();
        for _ in 0..200 {
            if started.load(Ordering::SeqCst) == MAX_RECEIPTS {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(started.load(Ordering::SeqCst), MAX_RECEIPTS);
        let response = router
            .oneshot(Request::post("/work").body(Body::empty()).expect("request"))
            .await
            .expect("capacity response");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), 4096).await.expect("body");
        let body: Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(body["parameters"]["operationOutcome"], "not_applied");
        for request in requests {
            assert_eq!(
                request.await.expect("request task").status(),
                StatusCode::ACCEPTED
            );
        }
        release.notify_waiters();
    }

    #[test]
    fn acknowledgement_and_expiry_remove_completed_receipts_only() {
        let clock = ManualClock::new(mixrelay_time::Instant::from_origin(Duration::from_mins(10)));
        let operations = ControlOperations::new(Box::new(clock.clone()));
        let pending = operations.admit(None).expect("pending admission");
        assert!(matches!(operations.acknowledge(&pending.id), Ack::Pending));
        operations.complete(&pending.id, stored(StatusCode::OK, b"ok", true));
        assert!(matches!(
            operations.acknowledge(&pending.id),
            Ack::Completed
        ));
        assert!(matches!(
            operations.snapshot(&pending.id),
            Snapshot::Missing
        ));

        let expired = operations.admit(None).expect("expiry admission");
        clock.advance(COMPLETED_TTL);
        assert!(matches!(
            operations.snapshot(&expired.id),
            Snapshot::Pending
        ));
        operations.complete(&expired.id, stored(StatusCode::OK, b"ok", true));
        clock.advance(COMPLETED_TTL.checked_sub(Duration::from_nanos(1)).unwrap());
        assert!(matches!(
            operations.snapshot(&expired.id),
            Snapshot::Complete(_)
        ));
        clock.advance(Duration::from_nanos(1));
        assert!(matches!(
            operations.snapshot(&expired.id),
            Snapshot::Missing
        ));
        // A late completion for a receipt that is gone changes nothing.
        operations.complete(&expired.id, stored(StatusCode::OK, b"late", true));
        assert_eq!(operations.lock_ledger().len(), 0);
    }

    #[tokio::test]
    async fn blocking_task_panic_is_uncertain_without_result_header() {
        let operations = fast_operations();
        let router = test_router(
            operations.clone(),
            post(|| async {
                tokio::time::sleep(SLOW_HANDLER).await;
                crate::catalog_task(|| panic!("fixture blocking task panic")).await
            }),
        );
        let response = router
            .oneshot(Request::post("/work").body(Body::empty()).expect("request"))
            .await
            .expect("response");
        let id = pending_id(response).await;
        let response = terminal_response(&operations, &id).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(response.headers().get(RESULT_HEADER).is_none());
        let body = to_bytes(response.into_body(), 4096).await.expect("body");
        let body: Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(body["parameters"]["operationOutcome"], "uncertain");
    }

    #[tokio::test]
    async fn read_admission_failure_has_no_mutation_outcome() {
        let response = admission_full_response(
            &Request::get(crate::GRAPH_CATALOG_ROUTE)
                .body(Body::empty())
                .expect("read request"),
        );
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), 4096).await.expect("body");
        let body: Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(body["parameters"], json!({}));
    }

    #[tokio::test]
    async fn admission_failure_uses_the_target_transaction_outcome() {
        for (path, outcome) in [
            (crate::GRAPH_CATALOG_ROUTE, "catalogOutcome"),
            ("/api/v1/workspace", "workspaceOutcome"),
            ("/api/v1/audio/power", "powerOutcome"),
            ("/api/v1/graphs/a/nodes/b/control", "nodeControlOutcome"),
            (crate::GRAPH_ROUTE, "operationOutcome"),
        ] {
            let request = Request::put(path).body(Body::empty()).expect("request");
            let response = admission_full_response(&request);
            let body = to_bytes(response.into_body(), 4096).await.expect("body");
            let body: Value = serde_json::from_slice(&body).expect("JSON");
            assert_eq!(
                body["parameters"],
                json!({outcome: "not_applied"}),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn panic_and_oversize_are_uncertain_without_result_header() {
        let operations = fast_operations();
        let router = test_router(
            operations.clone(),
            post(|| async {
                tokio::time::sleep(SLOW_HANDLER).await;
                panic_status()
            }),
        );
        let response = router
            .oneshot(Request::post("/work").body(Body::empty()).expect("request"))
            .await
            .expect("response");
        let id = pending_id(response).await;
        let response = terminal_response(&operations, &id).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response.headers().get(RESULT_HEADER).is_none());
        let body = to_bytes(response.into_body(), 4096).await.expect("body");
        let body: Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(body["parameters"]["operationOutcome"], "uncertain");

        let operations = fast_operations();
        let router = test_router(
            operations.clone(),
            post(|| async {
                tokio::time::sleep(SLOW_HANDLER).await;
                Body::from(vec![b'x'; RESPONSE_LIMIT + 1])
            }),
        );
        let response = router
            .oneshot(Request::post("/work").body(Body::empty()).expect("request"))
            .await
            .expect("response");
        let id = pending_id(response).await;
        let response = terminal_response(&operations, &id).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response.headers().get(RESULT_HEADER).is_none());
    }
}
