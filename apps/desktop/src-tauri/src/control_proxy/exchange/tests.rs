use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    task::JoinHandle,
};

use super::*;

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
    let result = request(&client, &origin, Method::PUT, "/graph", None)
        .await
        .expect("terminal response");
    assert_eq!(result.status, 202);
    assert_eq!(result.body["revision"], 12);
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
    assert_eq!(result.delivery, RequestDelivery::Uncertain);
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
    assert_eq!(result.delivery, RequestDelivery::NotSent);
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

#[test]
fn receipt_body_must_match_and_id_is_one_path_segment() {
    assert!(validate_receipt("one", &serde_json::json!({"operationId":"two"})).is_err());
    assert_eq!(
        operation_url("http://127.0.0.1", "a/b?c")
            .expect("url")
            .path(),
        "/api/v1/operations/a%2Fb%3Fc"
    );
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
        assert_eq!(result.delivery, RequestDelivery::ServerResponse);
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
        assert_eq!(result.delivery, RequestDelivery::Uncertain);
        assert_eq!(server.await.unwrap().len(), 2);
    }
}

#[test]
fn response_engine_survives_runtime_and_webview_serialization() {
    let unidentified = serde_json::json!({"status":200,"body":{}});
    let mut response: ControlProxyResponse = serde_json::from_value(unidentified.clone()).unwrap();
    assert!(response.engine.is_none());
    assert_eq!(serde_json::to_value(&response).unwrap(), unidentified);
    response.engine = Some("engine".into());
    let wire = serde_json::to_value(response).unwrap();
    assert_eq!(wire["engine"], "engine");
    assert_eq!(
        serde_json::from_value::<ControlProxyResponse>(wire)
            .unwrap()
            .engine
            .as_deref(),
        Some("engine")
    );
}

#[test]
fn client_operation_scope_matches_engine_dispatcher() {
    for path in [
        "/health",
        "/api/v1/status",
        "/api/v1/graph/status",
        "/api/v1/controls/snapshot",
        "/api/v1/meters",
        "/api/v1/operations",
        "/api/v1/operations/id",
        "/api/v1/processors/n/editor/frame",
        "/api/v1/processors/n/editor/input",
    ] {
        assert!(!needs_operation(&Method::GET, path));
        assert!(!needs_operation(&Method::POST, path));
    }
    for path in [
        "/api/v1/graph",
        "/api/v1/graphs",
        "/api/v1/graphs/g/layout",
        "/api/v1/processors",
        "/api/v1/application-capture/manifest",
    ] {
        assert!(needs_operation(&Method::GET, path));
    }
    assert!(needs_operation(&Method::PUT, "/api/v1/audio/power"));
    assert!(!needs_operation(&Method::GET, "/api/v1/telemetry/meters"));
    assert!(!needs_operation(&Method::DELETE, "/graph"));
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
    assert_eq!(result.delivery, RequestDelivery::ServerResponse);
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
        assert_eq!(result.delivery, RequestDelivery::Uncertain);
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
    assert_eq!(result.expect_err("foreign receipt"), IDENTITY_CHANGED);
    assert_eq!(server.await.unwrap().len(), 1);
}

#[tokio::test]
async fn fast_completion_of_another_operation_is_rejected_without_acknowledgement() {
    for status in [200, 204, 404] {
        let body = if status == 204 { "" } else { r#"{"ok":true}"# };
        let (origin, client, server) = server(vec![(status, Some("$foreign"), body)]);
        let result = request(&client, &origin, Method::PUT, "/graph", None).await;
        assert_eq!(result.expect_err("foreign result"), IDENTITY_CHANGED);
        // The fixture serves one connection; an acknowledgement would be a second.
        assert_eq!(server.await.unwrap().len(), 1);
    }
    let (origin, client, server) = server(vec![(200, Some("$foreign"), r#"{"ready":true}"#)]);
    let result = request(&client, &origin, Method::GET, "/health", None).await;
    assert_eq!(result.expect_err("unrequested result"), IDENTITY_CHANGED);
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
            assert_eq!(result.delivery, RequestDelivery::Uncertain);
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
fn terminal_404_requires_a_matching_result_header() {
    for (result_id, valid) in [
        (None, false),
        (Some("other-id"), false),
        (Some("client-id"), true),
    ] {
        let mut response = tauri::http::Response::builder().status(404);
        if let Some(id) = result_id {
            response = response.header("x-mixrelay-operation-result", id);
        }
        let response = Response::from(response.body("{}").unwrap());
        assert_eq!(
            validate_terminal(&response, Some("client-id")).is_ok(),
            valid
        );
    }
}

#[tokio::test]
async fn fully_received_invalid_json_is_not_recoverable_transport_failure() {
    let client = ControlProxyClient::with_socket(None);
    let response = Response::from(tauri::http::Response::new("{"));
    let result = finish(&client, "http://localhost", response, None, None).await;
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
