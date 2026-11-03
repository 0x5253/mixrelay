//! Native discovery must leave the controller's command and heartbeat tasks runnable.
use serde_json::json;

use super::discover;

#[tokio::test]
async fn blocking_discovery_allows_an_independent_controller_task_to_answer() {
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let controller = tokio::spawn(async move {
        started.await.unwrap();
        let _ = release.send(json!([{"uid": "synthetic-device"}]));
    });
    let result = discover(move || {
        entered.send(()).unwrap();
        // Only a deadlock guard: success requires another task on this one-thread
        // runtime to answer while the synchronous query is still waiting.
        released
            .recv_timeout(std::time::Duration::from_secs(1))
            .map_err(|_| json!({"code": "controller_task_starved"}))
    })
    .await;
    controller.await.unwrap();
    assert_eq!(result, Ok(json!([{"uid": "synthetic-device"}])));
}

#[tokio::test]
async fn native_discovery_failure_reaches_the_command_caller() {
    let result = discover(|| Err::<(), _>(json!({"code": "native_query_failed"}))).await;
    assert_eq!(result, Err(json!({"code": "native_query_failed"})));
}
