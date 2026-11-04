use crate::{ControlOperations, operations::IngressLease};
use axum::{
    Json,
    body::{Body, HttpBody, to_bytes},
    extract::{Extension, Request},
    http::{HeaderValue, StatusCode, header::CONNECTION},
    middleware::Next,
    response::{IntoResponse, Response},
};
use mixrelay_control_protocol::{ErrorCode, ProtocolError};

const BODY_LIMIT: usize = 16 * 1024 * 1024;

/// Bound ingress before handing ownership to a mutation. Once dispatched, a native
/// transaction or recorder finalization must complete even while the server shuts down.
pub(super) async fn read_body(
    Extension(operations): Extension<ControlOperations>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(mut response) = crate::operations::before_ingress(&operations, &request) {
        // The body was deliberately not drained; do not reuse this HTTP connection.
        response
            .headers_mut()
            .insert(CONNECTION, HeaderValue::from_static("close"));
        return response;
    }
    let (mut parts, body) = request.into_parts();
    let lease = if body.is_end_stream() {
        None
    } else if let Some(lease) = operations.try_acquire_ingress() {
        Some(lease)
    } else {
        let mut response =
            crate::operations::admission_full_response(&Request::from_parts(parts, body));
        response
            .headers_mut()
            .insert(CONNECTION, HeaderValue::from_static("close"));
        return response;
    };
    let bytes = match tokio::time::timeout(
        operations.timings().body_deadline,
        to_bytes(body, BODY_LIMIT),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => return rejected(StatusCode::BAD_REQUEST, &error.to_string()),
        Err(_) => {
            return rejected(
                StatusCode::REQUEST_TIMEOUT,
                "request body deadline exceeded",
            );
        }
    };
    if let Some(lease_for_request) = lease.clone() {
        parts.extensions.insert::<IngressLease>(lease_for_request);
    }
    let response = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    drop(lease);
    response
}

fn rejected(status: StatusCode, reason: &str) -> Response {
    (
        status,
        [(CONNECTION, "close")],
        Json(ProtocolError {
            code: ErrorCode::GraphInvalid,
            parameters: serde_json::json!({"reason": reason}),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    /// Short enough that deadline tests stay fast, long enough for a local request.
    const TEST_DEADLINE: Duration = Duration::from_millis(50);

    fn operations() -> crate::ControlOperations {
        crate::ControlOperations::new(crate::tests::clock()).with_timings(crate::OperationTimings {
            body_deadline: TEST_DEADLINE,
            ..crate::OperationTimings::default()
        })
    }

    fn test_limits() -> crate::SocketLimits {
        crate::SocketLimits {
            write_stall: TEST_DEADLINE,
            ..crate::SocketLimits::default()
        }
    }

    use axum::{
        Router, middleware,
        routing::{get, post},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        sync::{Notify, oneshot},
    };
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn incomplete_body_cannot_hold_graceful_shutdown_open() {
        let operations = operations();
        let entered = Arc::new(Notify::new());
        let observed = entered.clone();
        let router = crate::router(operations.clone()).layer(middleware::from_fn(
            move |request, next: Next| {
                let observed = observed.clone();
                async move {
                    observed.notify_one();
                    next.run(request).await
                }
            },
        ));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let (shutdown, stopped) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(crate::bounded_listener(listener, test_limits()), router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .expect("server");
        });
        let mut client = TcpStream::connect(address).await.expect("client");
        client.write_all(b"PUT /api/v1/audio/power HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{")
            .await.expect("partial request");
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .expect("request entered router");
        shutdown.send(()).expect("signal shutdown");
        tokio::time::timeout(TEST_DEADLINE + Duration::from_secs(2), server)
            .await
            .expect("shutdown must drain incomplete ingress")
            .expect("server task");
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), client.read_to_end(&mut response))
            .await
            .expect("connection closed")
            .expect("response");
        assert!(
            String::from_utf8(response)
                .expect("HTTP response")
                .starts_with("HTTP/1.1 408")
        );
        let restored: Vec<_> = (0..8)
            .map(|_| {
                operations
                    .try_acquire_ingress()
                    .expect("timeout releases ingress")
            })
            .collect();
        assert_eq!(restored.len(), 8);
    }

    #[tokio::test]
    async fn ninth_body_is_rejected_before_backend_admission() {
        async fn rejected_backend() -> StatusCode {
            panic!("rejected body reached backend")
        }
        let operations = operations();
        let held: Vec<_> = (0..8)
            .map(|_| operations.try_acquire_ingress().expect("budget permit"))
            .collect();
        let router = Router::new()
            .route("/api/v1/graphs", post(rejected_backend))
            .layer(middleware::from_fn(read_body))
            .layer(axum::Extension(operations));
        let response = router
            .oneshot(
                Request::post("/api/v1/graphs")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[CONNECTION], "close");
        let bytes = to_bytes(response.into_body(), 4096).await.expect("body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
        assert_eq!(body["parameters"]["catalogOutcome"], "not_applied");
        drop(held);
    }

    #[tokio::test]
    async fn disconnected_incomplete_body_releases_ingress() {
        let operations = operations();
        let held: Vec<_> = (0..7)
            .map(|_| operations.try_acquire_ingress().expect("budget permit"))
            .collect();
        let router = crate::router(operations.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(crate::bounded_listener(listener, test_limits()), router)
                .await
                .expect("server");
        });
        let mut client = TcpStream::connect(address).await.expect("client");
        client
            .write_all(
                b"PUT /api/v1/graphs HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\n\r\n{",
            )
            .await
            .expect("partial body");
        tokio::time::timeout(Duration::from_secs(1), async {
            while operations.try_acquire_ingress().is_some() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("body admitted");
        drop(client);
        let released = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(lease) = operations.try_acquire_ingress() {
                    break lease;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await;
        server.abort();
        let _ = server.await;
        assert!(released.is_ok(), "disconnected body must release admission");
        drop(held);
    }

    #[tokio::test]
    async fn ingress_deadline_does_not_cancel_an_accepted_mutation() {
        let started = Arc::new(Notify::new());
        let finish = Arc::new(Notify::new());
        let handler_started = started.clone();
        let handler_finish = finish.clone();
        let router = Router::new()
            .route(
                "/mutation",
                post(move || async move {
                    handler_started.notify_one();
                    handler_finish.notified().await;
                    StatusCode::NO_CONTENT
                }),
            )
            .layer(middleware::from_fn(read_body))
            .layer(axum::Extension(operations()));
        let task = tokio::spawn(
            router.oneshot(
                Request::post("/mutation")
                    .body(Body::from("{}"))
                    .expect("request"),
            ),
        );
        started.notified().await;
        tokio::time::sleep(TEST_DEADLINE + Duration::from_millis(20)).await;
        assert!(!task.is_finished(), "accepted work must retain its owner");
        finish.notify_one();
        assert_eq!(
            task.await.expect("task").expect("response").status(),
            StatusCode::NO_CONTENT
        );
    }

    #[tokio::test]
    async fn empty_body_bypasses_full_ingress_budget() {
        let operations = operations();
        let held: Vec<_> = (0..8)
            .map(|_| operations.try_acquire_ingress().expect("budget permit"))
            .collect();
        let router = Router::new()
            .route("/health", get(|| async { StatusCode::NO_CONTENT }))
            .layer(middleware::from_fn(read_body))
            .layer(axum::Extension(operations));
        let response = router
            .oneshot(
                Request::get("/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        drop(held);
    }

    #[tokio::test]
    async fn detached_operation_retains_ingress_lease_until_completion() {
        let operations = operations();
        let lease = operations.try_acquire_ingress().expect("budget permit");
        let held: Vec<_> = (0..7)
            .map(|_| operations.try_acquire_ingress().expect("budget permit"))
            .collect();
        let started = Arc::new(Notify::new());
        let finish = Arc::new(Notify::new());
        let handler_started = started.clone();
        let handler_finish = finish.clone();
        let router = Router::new()
            .route(
                "/work",
                post(move || async move {
                    handler_started.notify_one();
                    handler_finish.notified().await;
                    StatusCode::NO_CONTENT
                }),
            )
            .layer(middleware::from_fn(crate::operations::dispatch))
            .layer(axum::Extension(operations.clone()));
        let mut request = Request::post("/work").body(Body::empty()).expect("request");
        request.extensions_mut().insert(lease);
        let task = tokio::spawn(router.oneshot(request));
        started.notified().await;
        task.abort();
        assert!(task.await.expect_err("observer aborted").is_cancelled());
        assert!(
            operations.try_acquire_ingress().is_none(),
            "detached work must retain its ingress permit"
        );
        finish.notify_one();
        operations.drain().await;
        assert!(
            operations.try_acquire_ingress().is_some(),
            "completed detached work must release its ingress permit"
        );
        drop(held);
    }

    #[tokio::test]
    async fn non_reading_client_cannot_hold_graceful_shutdown_open() {
        let router = Router::new()
            .route("/large", post(|| async { vec![b'x'; 32 * 1024 * 1024] }))
            .layer(middleware::from_fn(read_body))
            .layer(axum::Extension(operations()));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let (shutdown, stopped) = oneshot::channel();
        let mut server = tokio::spawn(async move {
            axum::serve(crate::bounded_listener(listener, test_limits()), router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .expect("server");
        });
        let mut client = TcpStream::connect(address).await.expect("client");
        client
            .write_all(b"POST /large HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n")
            .await
            .expect("request");
        let mut first_byte = [0];
        tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut first_byte))
            .await
            .expect("response began")
            .expect("first byte");
        shutdown.send(()).expect("signal shutdown");
        let drained = tokio::time::timeout(Duration::from_secs(3), &mut server).await;
        let stalled = drained.is_err();
        // Always release the fixture peer before reporting a failed drain assertion.
        drop(client);
        match drained {
            Ok(result) => result.expect("server task"),
            Err(_) => tokio::time::timeout(Duration::from_secs(2), server)
                .await
                .expect("drain after peer disconnect")
                .expect("server task"),
        }
        assert!(
            !stalled,
            "non-reading peer held graceful shutdown past its write-stall limit"
        );
    }

    #[tokio::test]
    async fn socket_deadline_preserves_dispatched_mutation_during_shutdown() {
        let started = Arc::new(Notify::new());
        let finish = Arc::new(Notify::new());
        let handler_started = started.clone();
        let handler_finish = finish.clone();
        let router = Router::new()
            .route(
                "/mutation",
                post(move || async move {
                    handler_started.notify_one();
                    handler_finish.notified().await;
                    StatusCode::NO_CONTENT
                }),
            )
            .layer(middleware::from_fn(read_body))
            .layer(axum::Extension(operations()));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let (shutdown, stopped) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(crate::bounded_listener(listener, test_limits()), router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .expect("server");
        });
        let mut client = TcpStream::connect(address).await.expect("client");
        client
            .write_all(b"POST /mutation HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n")
            .await
            .expect("request");
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("mutation dispatched");
        shutdown.send(()).expect("signal shutdown");
        tokio::time::sleep(TEST_DEADLINE + Duration::from_millis(20)).await;
        assert!(
            !server.is_finished(),
            "shutdown must wait for accepted mutation"
        );
        finish.notify_one();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("mutation finalized")
            .expect("server task");
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), client.read_to_end(&mut response))
            .await
            .expect("connection closed")
            .expect("response");
        assert!(
            String::from_utf8(response)
                .expect("HTTP response")
                .starts_with("HTTP/1.1 204")
        );
    }
}
