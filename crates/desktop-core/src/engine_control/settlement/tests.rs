use super::*;
use serde_json::json;

const ALL: [Submission; 4] = [
    Submission::Graph,
    Submission::Workspace,
    Submission::Power,
    Submission::NodeControl,
];

#[test]
fn an_unsent_submission_releases_its_reservation() {
    for submission in ALL {
        assert_eq!(
            settle(submission, Delivery::NotSent, 409, &json!({}), 7),
            Ok(Settlement::Release)
        );
    }
}

#[test]
fn explicit_not_applied_outcomes_release_but_the_same_answer_lost_in_transit_retains() {
    for (submission, field) in [
        (Submission::Workspace, "workspaceOutcome"),
        (Submission::Power, "powerOutcome"),
        (Submission::NodeControl, "nodeControlOutcome"),
    ] {
        let body = json!({"code":"control_unavailable", "parameters":{field:"not_applied"}});
        assert_eq!(
            settle(submission, Delivery::ServerResponse, 503, &body, 7),
            Ok(Settlement::Release),
            "{field}"
        );
        assert_eq!(
            settle(submission, Delivery::Uncertain, 503, &body, 7),
            Ok(Settlement::Retain),
            "{field}"
        );
    }
}

#[test]
fn client_errors_release_but_server_errors_without_an_outcome_retain() {
    let body = json!({"code":"graph_exchange_busy"});
    assert_eq!(
        settle(Submission::Graph, Delivery::ServerResponse, 409, &body, 7),
        Ok(Settlement::Release)
    );
    for status in [500, 503] {
        assert_eq!(
            settle(
                Submission::Graph,
                Delivery::ServerResponse,
                status,
                &body,
                7
            ),
            Ok(Settlement::Retain)
        );
    }
    // Other outcome fields do not prove the capture-changing mutation was not applied.
    let catalog = json!({"code":"x", "parameters":{"catalogOutcome":"not_applied"}});
    assert_eq!(
        settle(
            Submission::Graph,
            Delivery::ServerResponse,
            503,
            &catalog,
            7
        ),
        Ok(Settlement::Retain)
    );
}

#[test]
fn acceptance_must_acknowledge_exactly_the_prepared_revision_where_the_route_reports_it() {
    for (submission, body) in [
        (Submission::Graph, json!({"revision":7})),
        (Submission::Power, json!({"revision":7})),
        (Submission::Workspace, json!({"document":{"revision":7}})),
        (
            Submission::NodeControl,
            json!({"application":{"revision":7}}),
        ),
    ] {
        assert_eq!(
            settle(submission, Delivery::ServerResponse, 200, &body, 7),
            Ok(Settlement::Accept),
            "{submission:?}"
        );
        for prepared in [0, 6, 8] {
            assert_eq!(
                settle(submission, Delivery::ServerResponse, 200, &body, prepared),
                Err(RevisionMismatch)
            );
        }
    }
    // A workspace answer's top-level revision is not the document's.
    assert_eq!(
        settle(
            Submission::Workspace,
            Delivery::ServerResponse,
            200,
            &json!({"revision":7}),
            7
        ),
        Err(RevisionMismatch)
    );
    assert_eq!(
        settle(
            Submission::Graph,
            Delivery::ServerResponse,
            200,
            &json!({}),
            7
        ),
        Err(RevisionMismatch)
    );
}

#[test]
fn a_retained_error_is_marked_pending_confirmation_only_where_it_has_parameters() {
    let mut body = json!({"code":"control_unavailable", "parameters":{"reason":"lost"}});
    mark_pending_confirmation(&mut body);
    assert_eq!(body["parameters"]["captureOutcome"], "pending_confirmation");
    assert_eq!(body["parameters"]["reason"], "lost");

    let mut bare = json!({"code":"control_unavailable"});
    mark_pending_confirmation(&mut bare);
    assert_eq!(bare, json!({"code":"control_unavailable"}));
}
