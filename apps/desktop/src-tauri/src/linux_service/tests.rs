//! Service admission has deterministic success, failure and deadline outcomes.
use super::*;
use std::{future, os::unix::process::ExitStatusExt as _};

#[tokio::test(start_paused = true)]
async fn service_failure_never_looks_like_ready_audio() {
    wait_for_start(future::ready(Ok(ExitStatus::from_raw(0))))
        .await
        .unwrap();
    assert!(matches!(
        wait_for_start(future::ready(Ok(ExitStatus::from_raw(1 << 8)))).await,
        Err(StartError::Refused(_))
    ));
    assert!(matches!(
        wait_for_start(future::ready(Err(std::io::Error::from(
            std::io::ErrorKind::NotFound
        ))))
        .await,
        Err(StartError::Io(_))
    ));
}

#[tokio::test(start_paused = true)]
async fn an_unresponsive_user_manager_cannot_hold_startup_forever() {
    let started = tokio::time::Instant::now();
    assert!(matches!(
        wait_for_start(future::pending()).await,
        Err(StartError::Timeout)
    ));
    assert_eq!(started.elapsed(), START_TIMEOUT);
}
