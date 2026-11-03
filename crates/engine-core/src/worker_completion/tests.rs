//! Worker errors and shutdown observers are independent, deterministic boundaries.
use super::*;
use std::future::{pending, ready};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
struct NativeFailure(&'static str);

#[tokio::test]
async fn worker_failure_stops_admission_and_survives_final_join() {
    let (sender, mut completion) = channel::<NativeFailure>();
    sender
        .send(Err(NativeFailure("native capture failed")))
        .unwrap();
    let error = completion.shutdown(pending()).await.unwrap_err();
    assert!(error.to_string().contains("native capture failed"));
    assert!(
        matches!(completion.finish().await, Err(Error::Audio(message)) if *message == NativeFailure("native capture failed"))
    );
}

#[tokio::test]
async fn shutdown_observer_cannot_hide_a_later_worker_error() {
    let (sender, mut completion) = channel::<NativeFailure>();
    let error = completion
        .shutdown(ready(Err(io::Error::other("signal observer failed"))))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("signal observer failed"));
    sender
        .send(Err(NativeFailure("native retirement failed")))
        .unwrap();
    assert!(
        matches!(completion.finish().await, Err(Error::Audio(message)) if *message == NativeFailure("native retirement failed"))
    );
}

#[tokio::test]
async fn requested_shutdown_and_successful_drain_are_successful() {
    let (sender, mut completion) = channel::<NativeFailure>();
    completion.shutdown(ready(Ok(()))).await.unwrap();
    sender.send(Ok(())).unwrap();
    completion.finish().await.unwrap();
}

#[tokio::test]
async fn unexpected_clean_exit_closes_control_admission() {
    let (sender, mut completion) = channel::<NativeFailure>();
    sender.send(Ok(())).unwrap();
    let error = completion.shutdown(pending()).await.unwrap_err();
    assert!(error.to_string().contains("exited before shutdown"));
    completion.finish().await.unwrap();
}

#[tokio::test]
async fn lost_worker_result_is_an_observable_failure() {
    let (sender, mut completion) = channel::<NativeFailure>();
    drop(sender);
    let error = completion.shutdown(pending()).await.unwrap_err();
    assert!(error.to_string().contains("without a result"));
    assert!(matches!(
        completion.finish().await,
        Err(Error::MissingResult)
    ));
}
