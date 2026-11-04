use super::*;
use serde_json::json;

fn headers<'a>(
    status: u16,
    receipt: Option<&'a str>,
    result: Option<&'a str>,
) -> ResponseHeaders<'a> {
    ResponseHeaders {
        status,
        receipt,
        result,
    }
}

#[test]
fn client_operation_scope_matches_the_engine_dispatcher() {
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
        assert!(!requires_client_operation("GET", path), "{path}");
        assert!(!requires_client_operation("POST", path), "{path}");
    }
    for path in [
        "/api/v1/graph",
        "/api/v1/graphs",
        "/api/v1/graphs/g/layout",
        "/api/v1/processors",
        "/api/v1/application-capture/manifest",
    ] {
        assert!(requires_client_operation("GET", path), "{path}");
    }
    assert!(requires_client_operation("PUT", "/api/v1/audio/power"));
    assert!(requires_client_operation("POST", "/api/v1/workspace"));
    assert!(!requires_client_operation(
        "GET",
        "/api/v1/telemetry/meters"
    ));
    assert!(!requires_client_operation("GET", "/api/v1/graphsx"));
    assert!(!requires_client_operation("DELETE", "/graph"));
}

#[test]
fn only_reads_of_one_receipt_are_receipt_observations() {
    assert!(is_receipt_observation("GET", "/api/v1/operations/ours"));
    assert!(!is_receipt_observation("GET", "/api/v1/operations"));
    assert!(!is_receipt_observation("DELETE", "/api/v1/operations/ours"));
    assert!(!is_receipt_observation("GET", "/api/v1/operationsx/ours"));
}

#[test]
fn the_editors_operation_id_leaves_the_body_but_a_preview_never_adopts_it() {
    let mut body = json!({"clientOperationId":"ui-consent", "revision":2});
    assert_eq!(
        take_client_operation_id(Some(&mut body), "/api/v1/graph").as_deref(),
        Some("ui-consent")
    );
    assert_eq!(body, json!({"revision":2}));

    let mut preview = json!({"clientOperationId":"ui-consent", "revision":2});
    assert_eq!(
        take_client_operation_id(Some(&mut preview), "/api/v1/application-capture/preview"),
        None
    );
    assert_eq!(preview, json!({"revision":2}));

    let mut numeric = json!({"clientOperationId":7});
    assert_eq!(
        take_client_operation_id(Some(&mut numeric), "/api/v1/graph"),
        None
    );
    assert_eq!(numeric, json!({}));
    assert_eq!(take_client_operation_id(None, "/api/v1/graph"), None);
    assert_eq!(
        take_client_operation_id(Some(&mut json!([1])), "/api/v1/graph"),
        None
    );
}

#[test]
fn a_receipt_for_the_requested_id_is_polled_and_any_other_is_refused() {
    assert_eq!(
        headers(202, Some("ours"), None).next(Some("ours"), None),
        Ok(Next::Poll("ours".into()))
    );
    for requested in [Some("ours"), None] {
        assert_eq!(
            headers(202, Some("server-issued"), None).next(requested, None),
            Err(ReceiptError::IdentityChanged)
        );
    }
    // A later receipt must keep the accepted identity too.
    assert_eq!(
        headers(202, Some("different"), None).next(Some("ours"), Some("ours")),
        Err(ReceiptError::IdentityChanged)
    );
}

#[test]
fn malformed_receipts_are_invalid_whatever_their_id() {
    let long = "a".repeat(129);
    for (status, receipt) in [(200, "ours"), (202, ""), (202, long.as_str())] {
        assert_eq!(
            headers(status, Some(receipt), None).next(Some(receipt), None),
            Err(ReceiptError::InvalidReceipt),
            "{status} {receipt}"
        );
    }
    let longest = "a".repeat(128);
    assert_eq!(
        headers(202, Some(&longest), None).next(Some(&longest), None),
        Ok(Next::Poll(longest.clone()))
    );
}

#[test]
fn a_quick_accepted_status_without_receipt_is_the_outcome() {
    assert_eq!(
        headers(202, None, None).next(Some("ours"), None),
        Ok(Next::Complete { acknowledge: None })
    );
}

#[test]
fn a_fast_completion_of_our_id_is_acknowledged_and_another_is_refused() {
    for status in [200, 204, 404] {
        assert_eq!(
            headers(status, None, Some("ours")).next(Some("ours"), None),
            Ok(Next::Complete {
                acknowledge: Some("ours".into())
            })
        );
        assert_eq!(
            headers(status, None, Some("foreign")).next(Some("ours"), None),
            Err(ReceiptError::IdentityChanged)
        );
    }
    // A read without a client ID never owns a stored result.
    assert_eq!(
        headers(200, None, Some("foreign")).next(None, None),
        Err(ReceiptError::IdentityChanged)
    );
}

#[test]
fn after_a_receipt_only_the_accepted_result_ends_the_wait() {
    assert_eq!(
        headers(409, None, Some("ours")).next(Some("ours"), Some("ours")),
        Ok(Next::Complete {
            acknowledge: Some("ours".into())
        })
    );
    for result in [None, Some("other")] {
        assert_eq!(
            headers(404, None, result).next(Some("ours"), Some("ours")),
            Err(ReceiptError::Unconfirmed)
        );
    }
}

#[test]
fn a_receipt_lookup_concerns_us_only_through_our_result_or_receipt() {
    assert!(headers(404, None, Some("ours")).concerns("ours"));
    assert!(headers(202, Some("ours"), None).concerns("ours"));
    assert!(!headers(404, None, None).concerns("ours"));
    assert!(!headers(404, None, Some("other")).concerns("ours"));
    assert!(!headers(202, Some("other"), None).concerns("ours"));
    // A malformed receipt is not ours even when it repeats our ID.
    assert!(!headers(200, Some("ours"), None).concerns("ours"));
}

#[test]
fn a_receipt_body_must_name_the_receipt() {
    assert_eq!(
        confirm_receipt_body("one", &json!({"operationId":"one"})),
        Ok(())
    );
    for body in [json!({"operationId":"two"}), json!({}), json!(null)] {
        assert_eq!(
            confirm_receipt_body("one", &body),
            Err(ReceiptError::IdentityChanged)
        );
    }
}
