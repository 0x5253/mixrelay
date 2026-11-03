//! Retries recovery only while the controller still owns the audio lifetime.

use std::future::Future;

/// Observation failures never authorize replacing a potentially live process.
///
/// `recover` returns a future that owns what it needs: a future borrowing the caller's
/// state through an async closure cannot be proven `Send` for a spawned task.
pub(crate) async fn supervise<E: std::fmt::Display, R: Future<Output = Result<(), E>>>(
    stopping: impl Fn() -> bool,
    exited: impl FnMut() -> Result<bool, E>,
    recover: impl FnMut() -> R,
) {
    mixrelay_desktop_core::lifecycle::supervise(
        &crate::async_time::TokioDelay::new(mixrelay_time::StdClock::new()),
        stopping,
        exited,
        recover,
        |failure| match failure {
            mixrelay_desktop_core::lifecycle::SupervisionFailure::Observation(error) => {
                tracing::error!(%error, "could not observe owned audio engine");
            }
            mixrelay_desktop_core::lifecycle::SupervisionFailure::Recovery(error) => {
                tracing::error!(%error, "owned audio engine recovery failed; retrying");
            }
        },
    )
    .await;
}
