use std::sync::Mutex;

#[cfg(target_os = "macos")]
use mixrelay_desktop_core::engine_control::Delivery;
use mixrelay_desktop_core::engine_control::receipt::ReceiptError;
use mixrelay_time::{Instant, MonotonicClock};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    task::JoinHandle,
};

use super::*;

/// Receipt polls proceed at once and record the interval they would have waited.
#[derive(Default)]
struct PollTimer(Mutex<Vec<Duration>>);

impl MonotonicClock for PollTimer {
    fn now(&self) -> Instant {
        Instant::from_origin(Duration::ZERO)
    }
}

impl AsyncDelay for PollTimer {
    async fn wait_until(&self, deadline: Deadline) {
        self.0.lock().unwrap().push(deadline.remaining(self.now()));
    }
}

async fn request(
    client: &ControlProxyClient,
    origin: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<ControlProxyResponse, String> {
    Exchange::new(client, origin, &PollTimer::default())
        .request(method, path, body)
        .await
}

/// A one-shot engine on a socket in its own fixture directory, and a client of it.
fn server(
    replies: Vec<(u16, Option<&'static str>, &'static str)>,
) -> (String, ControlProxyClient, JoinHandle<Vec<String>>) {
    server_with_engines(replies, Vec::new())
}

// Status 0 drops the connection after reading the request. $client echoes its ID;
// $complete returns a terminal result bearing that ID (including fast completions);
// $foreign returns a terminal result bearing another operation's ID.
fn server_with_engines(
    replies: Vec<(u16, Option<&'static str>, &'static str)>,
    engines: Vec<Option<&'static str>>,
) -> (String, ControlProxyClient, JoinHandle<Vec<String>>) {
    let fixture = mixrelay_test_support::FixtureDir::ipc("exc").expect("socket fixture");
    let path = fixture.join("engine.sock");
    let listener = UnixListener::bind(&path).expect("bind fixture");
    let mut client = ControlProxyClient::with_socket(Some(path.clone()));
    if replies.iter().any(|(_, _, body)| *body == "$timeout") {
        client.client = reqwest::Client::builder()
            .unix_socket(path)
            .no_proxy()
            .retry(reqwest::retry::never())
            .read_timeout(Duration::from_millis(100))
            .build()
            .unwrap();
    }
    let origin = super::super::CONTROL_ORIGIN.to_owned();
    let task = tokio::spawn(async move {
        let _fixture = fixture;
        let mut requests = Vec::new();
        let mut accepted: Option<String> = None;
        let mut client_id = None;
        for (index, (status, receipt, body)) in replies.into_iter().enumerate() {
            let (mut socket, _) = listener.accept().await.expect("accept request");
            let request = read_request(&mut socket).await;
            if let Some(id) = header(&request, "x-mixrelay-operation-request") {
                client_id = Some(id.to_owned());
            }
            requests.push(request);
            if status == 0 {
                continue;
            }
            let explicit_result = matches!(receipt, Some("$complete" | "$foreign"));
            if explicit_result {
                let foreign = (receipt == Some("$foreign")).then(|| "foreign-operation".into());
                accepted = foreign.or_else(|| client_id.clone());
            }
            let receipt = match receipt {
                Some("$client") => client_id.clone(),
                Some("$complete" | "$foreign") | None => None,
                Some(id) => Some(id.to_owned()),
            };
            let header = receipt
                .as_ref()
                .map_or_else(String::new, |id| format!("X-MixRelay-Operation: {id}\r\n"));
            let result = if receipt.is_none() && (status != 404 || explicit_result) {
                accepted.as_ref().map_or_else(String::new, |id| {
                    format!("X-MixRelay-Operation-Result: {id}\r\n")
                })
            } else {
                String::new()
            };
            if receipt.is_some() {
                accepted = receipt;
            }
            let engine = engines
                .get(index)
                .copied()
                .flatten()
                .map_or_else(String::new, |engine| {
                    format!("X-MixRelay-Engine: {engine}\r\n")
                });
            let truncated = matches!(body, "$truncate" | "$timeout");
            let stalled = body == "$timeout";
            let body = if truncated { "{" } else { body };
            let body = body.replace("$client", client_id.as_deref().unwrap_or(""));
            let response = format!(
                "HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{header}{result}{engine}\r\n{body}",
                body.len() + if truncated { 64 } else { 0 }
            );
            socket.write_all(response.as_bytes()).await.expect("reply");
            if stalled {
                // Keep the incomplete body open until the client's read deadline closes it.
                let _ = socket.read(&mut [0]).await;
            }
        }
        requests
    });
    (origin, client, task)
}

async fn read_request(socket: &mut UnixStream) -> String {
    let mut request = Vec::new();
    loop {
        let mut byte = [0];
        socket.read_exact(&mut byte).await.expect("request header");
        request.push(byte[0]);
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
        assert!(request.len() < 8192, "bounded fixture request");
    }
    String::from_utf8(request).expect("HTTP header")
}

#[tokio::test]
async fn polls_once_submitted_work_and_preserves_terminal_accepted_status() {
    let (origin, client, server) = server(vec![
        (202, Some("$client"), r#"{"operationId":"$client"}"#),
        (202, Some("$client"), r#"{"operationId":"$client"}"#),
        (202, None, r#"{"revision":12,"state":"pending"}"#),
        (204, None, ""),
    ]);
    let timer = PollTimer::default();
    let result = Exchange::new(&client, &origin, &timer)
        .request(Method::PUT, "/graph", None)
        .await
        .expect("terminal response");
    assert_eq!(result.status, 202);
    assert_eq!(result.body["revision"], 12);
    assert_eq!(*timer.0.lock().unwrap(), [RECEIPT_POLL_INTERVAL; 2]);
    let requests = server.await.expect("fixture finished");
    assert_eq!(requests.len(), 4);
    assert!(requests[0].starts_with("PUT /graph "));
    let id = header(&requests[0], "x-mixrelay-operation-request").expect("client ID");
    assert!(requests[1].starts_with(&format!("GET /api/v1/operations/{id} ")));
    assert!(requests[2].starts_with(&format!("GET /api/v1/operations/{id} ")));
    assert!(requests[3].starts_with(&format!("DELETE /api/v1/operations/{id} ")));
}

#[tokio::test]
async fn quick_accepted_status_is_not_an_operation_receipt() {
    let (origin, client, server) = server(vec![(202, None, r#"{"revision":4}"#)]);
    let result = request(&client, &origin, Method::PUT, "/graph", None)
        .await
        .expect("response");
    assert_eq!(result.body["revision"], 4);
    assert_eq!(server.await.expect("fixture").len(), 1);
}

#[tokio::test]
async fn failed_acknowledgement_preserves_terminal_error() {
    let (origin, client, server) = server(vec![
        (202, Some("$client"), r#"{"operationId":"$client"}"#),
        (409, None, r#"{"code":"graph_exchange_busy"}"#),
    ]);
    let result = request(&client, &origin, Method::PUT, "/graph", None)
        .await
        .expect("terminal response survives lost acknowledgement");
    assert_eq!(result.status, 409);
    assert_eq!(result.body["code"], "graph_exchange_busy");
    assert_eq!(server.await.expect("fixture").len(), 2);
}

#[tokio::test]
async fn lost_poll_is_uncertain_even_when_connect_fails() {
    let (origin, client, server) =
        server(vec![(202, Some("$client"), r#"{"operationId":"$client"}"#)]);
    let result = request(&client, &origin, Method::PUT, "/graph", None)
        .await
        .expect("structured failure");
    assert_eq!(result.status, 503);
    #[cfg(target_os = "macos")]
    assert_eq!(result.delivery, Delivery::Uncertain);
    assert!(
        result.body["parameters"]
            .get("nodeControlOutcome")
            .is_none()
    );
    assert_eq!(server.await.expect("fixture").len(), 1);
}

#[tokio::test]
async fn initial_connection_refusal_is_distinct_from_a_lost_poll() {
    let (origin, client, server) = server(Vec::new());
    server.await.expect("listener closed");
    let result = request(&client, &origin, Method::PUT, "/graph", None)
        .await
        .expect("structured failure");
    assert_eq!(result.status, 503);
    #[cfg(target_os = "macos")]
    assert_eq!(result.delivery, Delivery::NotSent);
}

#[tokio::test]
async fn changed_receipt_is_rejected_without_resubmitting() {
    let (origin, client, server) = server(vec![
        (202, Some("$client"), r#"{"operationId":"$client"}"#),
        (202, Some("different"), r#"{"operationId":"different"}"#),
    ]);
    assert!(
        request(&client, &origin, Method::PUT, "/graph", None)
            .await
            .is_err()
    );
    assert_eq!(server.await.expect("fixture").len(), 2);
}

#[tokio::test]
async fn unknown_receipt_is_not_a_definitive_mutation_rejection() {
    let (origin, client, server) = server(vec![
        (202, Some("$client"), r#"{"operationId":"$client"}"#),
        (404, None, r#"{"code":"control_unavailable"}"#),
    ]);
    let result = request(&client, &origin, Method::PUT, "/graph", None).await;
    assert!(
        result
            .expect_err("unconfirmed outcome")
            .contains("could not be confirmed")
    );
    assert_eq!(server.await.expect("fixture").len(), 2);
}

fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
}

#[tokio::test]
async fn client_ids_engine_fences_and_fast_acknowledgements_are_forwarded() {
    let (origin, client, server) = server_with_engines(
        vec![
            (200, None, r#"{"ready":true}"#),
            (
                409,
                None,
                r#"{"code":"engine_replaced","parameters":{"operationOutcome":"not_admitted"}}"#,
            ),
            (200, Some("$complete"), r#"{"ok":true}"#),
            (204, None, ""),
        ],
        vec![Some("old"), Some("new"), Some("new"), Some("latest")],
    );
    assert_eq!(
        request(&client, &origin, Method::GET, "/health", None)
            .await
            .unwrap()
            .engine
            .as_deref(),
        Some("old")
    );
    let rejected = request(&client.clone(), &origin, Method::PUT, "/graph", None)
        .await
        .unwrap();
    assert_eq!(rejected.status, 409);
    assert_eq!(rejected.engine.as_deref(), Some("new"));
    assert_eq!(
        rejected.body["parameters"]["operationOutcome"],
        "not_admitted"
    );
    let done = request(&client, &origin, Method::POST, "/graph", None)
        .await
        .unwrap();
    assert_eq!(done.engine.as_deref(), Some("new"));
    assert_eq!(client.engine.lock().unwrap().as_deref(), Some("latest"));
    let requests = server.await.unwrap();
    assert!(header(&requests[0], "x-mixrelay-operation-request").is_none());
    assert_eq!(
        header(&requests[1], "x-mixrelay-expected-engine"),
        Some("old")
    );
    assert_eq!(
        header(&requests[2], "x-mixrelay-expected-engine"),
        Some("new")
    );
    let ids: Vec<_> = requests[1..3]
        .iter()
        .map(|request| header(request, "x-mixrelay-operation-request").unwrap())
        .collect();
    assert_ne!(ids[0], ids[1]);
    for id in &ids {
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
    }
    assert!(requests[3].starts_with(&format!("DELETE /api/v1/operations/{} ", ids[1])));
}

#[tokio::test]
async fn lost_send_resolves_completed_or_pending_client_id_without_replay() {
    for pending in [false, true] {
        let mut replies = vec![(0, None, "")];
        if pending {
            replies.push((202, Some("$client"), r#"{"operationId":"$client"}"#));
        }
        replies.extend([
            (200, Some("$complete"), r#"{"applied":true}"#),
            (204, None, ""),
        ]);
        let (origin, client, server) = server_with_engines(replies, vec![None, Some("engine")]);
        let result = request(&client, &origin, Method::PUT, "/graph", None)
            .await
            .unwrap();
        assert_eq!(result.status, 200);
        assert_eq!(result.body["applied"], true);
        #[cfg(target_os = "macos")]
        assert_eq!(result.delivery, Delivery::ServerResponse);
        let requests = server.await.unwrap();
        let id = header(&requests[0], "x-mixrelay-operation-request").unwrap();
        assert!(header(&requests[0], "x-mixrelay-expected-engine").is_none());
        assert!(requests[1].starts_with(&format!("GET /api/v1/operations/{id} ")));
        assert_eq!(requests.iter().filter(|r| r.starts_with("PUT ")).count(), 1);
        assert_eq!(requests.len(), if pending { 4 } else { 3 });
        assert_eq!(client.engine.lock().unwrap().as_deref(), Some("engine"));
    }
}

#[tokio::test]
async fn lost_send_missing_and_other_resolution_failures_are_uncertain() {
    for (status, receipt, body) in [
        (404, None, r#"{"code":"control_unavailable"}"#),
        (503, None, r#"{"code":"control_unavailable"}"#),
        (202, Some("wrong-id"), r#"{"operationId":"wrong-id"}"#),
        (0, None, ""),
    ] {
        let (origin, client, server) = server(vec![(0, None, ""), (status, receipt, body)]);
        let result = request(&client, &origin, Method::PUT, "/graph", None)
            .await
            .unwrap();
        assert_eq!(result.status, 503);
        assert_eq!(result.body["parameters"]["operationOutcome"], "uncertain");
        assert_eq!(
            result.body["parameters"]["diagnostic"]["boundary"],
            "runtime_engine"
        );
        assert_eq!(result.body["parameters"]["diagnostic"]["stage"], "request");
        assert_eq!(
            result.body["parameters"]["diagnostic"]["delivery"],
            "uncertain"
        );
        #[cfg(target_os = "macos")]
        assert_eq!(result.delivery, Delivery::Uncertain);
        assert_eq!(server.await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn empty_completed_response_is_returned_and_acknowledged() {
    let (origin, client, server) = server(vec![(204, Some("$complete"), ""), (204, None, "")]);
    let response = request(&client, &origin, Method::PUT, "/graph", None)
        .await
        .unwrap();
    assert_eq!(response.status, 204);
    assert_eq!(response.body, Value::Null);
    assert_eq!(server.await.unwrap().len(), 2);
}

#[tokio::test]
async fn lost_send_returns_stored_terminal_404_and_acknowledges_it() {
    let (origin, client, server) = server(vec![
        (0, None, ""),
        (404, Some("$complete"), r#"{"code":"node_not_found"}"#),
        (204, None, ""),
    ]);
    let result = request(&client, &origin, Method::PUT, "/graph", None)
        .await
        .unwrap();
    assert_eq!(result.status, 404);
    assert_eq!(result.body["code"], "node_not_found");
    #[cfg(target_os = "macos")]
    assert_eq!(result.delivery, Delivery::ServerResponse);
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].starts_with("GET /api/v1/operations/"));
    assert!(requests[2].starts_with("DELETE /api/v1/operations/"));
}

#[tokio::test]
async fn missing_receipt_after_restart_retains_uncertainty() {
    for replacement in [None, Some("replacement")] {
        let (origin, client, server) = server_with_engines(
            vec![
                (0, None, ""),
                (404, None, r#"{"code":"control_unavailable"}"#),
            ],
            vec![None, replacement],
        );
        *client.engine.lock().unwrap() = Some("original".into());
        let result = request(&client, &origin, Method::PUT, "/graph", None)
            .await
            .unwrap();
        assert_eq!(result.status, 503);
        assert_eq!(result.engine.as_deref(), replacement);
        assert_eq!(result.body["parameters"]["operationOutcome"], "uncertain");
        #[cfg(target_os = "macos")]
        assert_eq!(result.delivery, Delivery::Uncertain);
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].starts_with("GET /api/v1/operations/"));
    }
}

#[tokio::test]
async fn body_transport_failures_resolve_once_without_resubmission() {
    for failure in ["$truncate", "$timeout"] {
        for pending in [false, true] {
            let first = if pending {
                (202, Some("$client"), failure)
            } else {
                (200, Some("$complete"), failure)
            };
            let (origin, client, server) = server(vec![
                first,
                (200, Some("$complete"), r#"{"applied":true}"#),
                (204, None, ""),
            ]);
            let result = request(&client, &origin, Method::PUT, "/graph", None)
                .await
                .unwrap();
            assert_eq!(result.body["applied"], true);
            let requests = server.await.unwrap();
            let id = header(&requests[0], "x-mixrelay-operation-request").unwrap();
            assert_eq!(requests.len(), 3);
            assert!(requests[1].starts_with(&format!("GET /api/v1/operations/{id} ")));
            assert!(requests[2].starts_with(&format!("DELETE /api/v1/operations/{id} ")));
        }
    }
}

#[tokio::test]
async fn receipt_other_than_the_client_id_is_rejected_without_polling() {
    let (origin, client, server) = server(vec![(
        202,
        Some("server-issued"),
        r#"{"operationId":"server-issued"}"#,
    )]);
    let result = request(&client, &origin, Method::PUT, "/graph", None).await;
    assert_eq!(
        result.expect_err("foreign receipt"),
        ReceiptError::IdentityChanged.to_string()
    );
    assert_eq!(server.await.unwrap().len(), 1);
}

#[tokio::test]
async fn fast_completion_of_another_operation_is_rejected_without_acknowledgement() {
    for status in [200, 204, 404] {
        let body = if status == 204 { "" } else { r#"{"ok":true}"# };
        let (origin, client, server) = server(vec![(status, Some("$foreign"), body)]);
        let result = request(&client, &origin, Method::PUT, "/graph", None).await;
        assert_eq!(
            result.expect_err("foreign result"),
            ReceiptError::IdentityChanged.to_string()
        );
        // The fixture serves one connection; an acknowledgement would be a second.
        assert_eq!(server.await.unwrap().len(), 1);
    }
    let (origin, client, server) = server(vec![(200, Some("$foreign"), r#"{"ready":true}"#)]);
    let result = request(&client, &origin, Method::GET, "/health", None).await;
    assert_eq!(
        result.expect_err("unrequested result"),
        ReceiptError::IdentityChanged.to_string()
    );
    assert_eq!(server.await.unwrap().len(), 1);
}

#[tokio::test]
async fn recovery_body_failure_is_uncertain_and_does_not_lookup_again() {
    for first in [(0, None, ""), (200, Some("$complete"), "$truncate")] {
        for failure in ["$truncate", "$timeout"] {
            let (origin, client, server) = server(vec![first, (200, Some("$complete"), failure)]);
            let result = request(&client, &origin, Method::PUT, "/graph", None)
                .await
                .unwrap();
            assert_eq!(result.status, 503);
            assert_eq!(result.body["parameters"]["operationOutcome"], "uncertain");
            assert_eq!(
                result.body["parameters"]["diagnostic"]["stage"],
                "receipt_observation"
            );
            #[cfg(target_os = "macos")]
            assert_eq!(result.delivery, Delivery::Uncertain);
            let requests = server.await.unwrap();
            assert_eq!(requests.len(), 2);
            assert!(requests[1].starts_with("GET /api/v1/operations/"));
        }
    }
}

#[tokio::test]
async fn invalid_json_is_not_a_transport_failure() {
    let (origin, client, server) = server(vec![(200, Some("$complete"), "{")]);
    let error = request(&client, &origin, Method::PUT, "/graph", None)
        .await
        .unwrap_err();
    assert!(error.contains("response was invalid"));
    assert_eq!(server.await.unwrap().len(), 1);
}

#[test]
fn an_unreadable_receipt_is_invalid_and_an_unreadable_result_is_absent() {
    let unreadable = reqwest::header::HeaderValue::from_bytes(b"\xff").unwrap();
    let response = Response::from(
        tauri::http::Response::builder()
            .status(202)
            .header(RECEIPT_HEADER, unreadable.clone())
            .header(RESULT_HEADER, unreadable)
            .body("{}")
            .unwrap(),
    );
    let headers = response_headers(&response);
    assert_eq!(headers.receipt, Some(""));
    assert_eq!(headers.result, None);
    assert_eq!(
        headers.next(Some("ours"), None),
        Err(ReceiptError::InvalidReceipt)
    );
}

#[tokio::test]
async fn fully_received_invalid_json_is_not_recoverable_transport_failure() {
    let client = ControlProxyClient::with_socket(None);
    let response = Response::from(tauri::http::Response::new("{"));
    let result = Exchange::new(&client, "http://localhost", &PollTimer::default())
        .finish(response, None, None)
        .await;
    assert!(
        matches!(result, Err(ExchangeError::Invalid(reason)) if reason.contains("response was invalid"))
    );
}

#[tokio::test]
async fn receipt_observation_does_not_poll_or_acknowledge_the_writers_result() {
    let (origin, client, server) = server(vec![(202, Some("ours"), r#"{"operationId":"ours"}"#)]);
    let result = request(
        &client,
        &origin,
        Method::GET,
        "/api/v1/operations/ours",
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.status, 202);
    assert_eq!(result.body["operationId"], "ours");
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /api/v1/operations/ours "));
}

#[tokio::test]
async fn ui_operation_identity_survives_native_preparation_without_reusing_the_preview_receipt() {
    let (origin, client, server) = server(vec![
        (200, Some("$complete"), "{}"),
        (204, None, ""),
        (200, Some("$complete"), "{}"),
        (204, None, ""),
    ]);
    let body = Some(serde_json::json!({"clientOperationId":"ui-consent", "revision":2}));
    request(
        &client,
        &origin,
        Method::POST,
        "/api/v1/application-capture/preview",
        body.clone(),
    )
    .await
    .unwrap();
    request(&client, &origin, Method::PUT, "/api/v1/graph", body)
        .await
        .unwrap();
    let requests = server.await.unwrap();
    assert!(!requests[0].contains("x-mixrelay-operation-request: ui-consent"));
    assert!(requests[2].contains("x-mixrelay-operation-request: ui-consent"));
}
