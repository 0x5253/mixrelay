//! Whether the proxy may send a request now. Admitted exchanges hold a shared guard, so
//! recovery and shutdown can wait for every request already on its way to the engine.
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{RwLock, RwLockReadGuard};

#[derive(Debug)]
pub(super) struct Admission {
    /// Without an engine socket nothing is ever admitted or sent elsewhere.
    connected: bool,
    /// Quit is irreversible.
    closing: AtomicBool,
    /// The engine is being replaced; requests are refused until its successor is ready.
    unavailable: AtomicBool,
    requests: RwLock<()>,
}

impl Admission {
    pub(super) fn new(connected: bool) -> Self {
        Self {
            connected,
            closing: AtomicBool::new(false),
            unavailable: AtomicBool::new(false),
            requests: RwLock::new(()),
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.connected
            && !self.closing.load(Ordering::Acquire)
            && !self.unavailable.load(Ordering::Acquire)
    }

    /// Admits one exchange, which keeps its guard until it has an outcome; `None` when
    /// requests are refused.
    pub(super) async fn enter(&self) -> Option<RwLockReadGuard<'_, ()>> {
        let admitted = self.requests.read().await;
        self.is_open().then_some(admitted)
    }

    #[cfg(any(test, target_os = "linux"))]
    pub(super) fn suspend(&self) {
        self.unavailable.store(true, Ordering::Release);
    }

    /// Reopens after a suspension once every exchange admitted before it has finished.
    /// Only the lifecycle owner calls this, after obtaining the replacement engine's
    /// readiness acknowledgement. Returns `false` once shutdown has begun.
    #[cfg(any(test, target_os = "linux"))]
    pub(super) async fn resume(&self) -> bool {
        let _drained = self.requests.write().await;
        if self.closing.load(Ordering::Acquire) {
            return false;
        }
        self.unavailable.store(false, Ordering::Release);
        true
    }

    pub(super) fn close(&self) {
        self.closing.store(true, Ordering::Release);
    }

    /// Resolves once no admitted exchange remains.
    pub(super) async fn quiesce(&self) {
        drop(self.requests.write().await);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn without_an_engine_socket_nothing_is_admitted() {
        let admission = Admission::new(false);
        assert!(!admission.is_open());
        assert!(admission.enter().await.is_none());
    }

    #[tokio::test]
    async fn recovery_reopens_a_suspension_but_never_a_shutdown() {
        let admission = Admission::new(true);
        assert!(admission.enter().await.is_some());
        admission.suspend();
        assert!(admission.enter().await.is_none());
        assert!(admission.resume().await);
        assert!(admission.enter().await.is_some());

        admission.suspend();
        admission.close();
        assert!(!admission.resume().await);
        assert!(admission.enter().await.is_none());
    }

    #[tokio::test]
    async fn recovery_waits_for_admitted_requests_and_shutdown_wins() {
        let admission = std::sync::Arc::new(Admission::new(true));
        let admitted = admission.enter().await.expect("admitted");
        admission.suspend();
        let recovery = tokio::spawn({
            let admission = std::sync::Arc::clone(&admission);
            async move { admission.resume().await }
        });
        tokio::task::yield_now().await;
        assert!(!recovery.is_finished());
        admission.close();
        drop(admitted);
        assert!(!recovery.await.expect("recovery task"));
    }

    #[tokio::test(start_paused = true)]
    async fn quiescing_waits_for_every_admitted_exchange() {
        let admission = Admission::new(true);
        let admitted = admission.enter().await.expect("admitted");
        admission.close();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), admission.quiesce())
                .await
                .is_err()
        );
        drop(admitted);
        admission.quiesce().await;
    }
}
