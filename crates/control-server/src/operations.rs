//! Bounded long-running control operations for one server lifetime.
//!
//! A mutation is admitted under a receipt, runs detached from its HTTP caller, and
//! either replies within the completion wait or becomes a pending operation that
//! clients poll and acknowledge.
use std::{
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use mixrelay_time::MonotonicClock;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, watch};

mod completion;
mod dispatch;
mod engine_fence;
mod ledger;
mod receipt_routes;
mod responses;
mod timings;

pub(crate) use dispatch::{before_ingress, dispatch};
pub(crate) use engine_fence::{engine_header, engine_identity};
use ledger::{Ack, ReceiptLedger, Rejection, Snapshot};
pub(crate) use receipt_routes::{delete, get};
use responses::StoredResponse;
pub(crate) use responses::{admission_full_response, task_failure};
pub use timings::OperationTimings;

const MAX_RECEIPTS: usize = 8;
const INGRESS_PERMITS: usize = 8;
const COMPLETED_TTL: Duration = Duration::from_mins(1);

/// Owns bounded long-running control operations for one server lifetime.
#[derive(Clone)]
pub struct ControlOperations {
    inner: Arc<Inner>,
    timings: OperationTimings,
}

struct Inner {
    ledger: Mutex<ReceiptLedger<Box<StoredResponse>, Arc<Notify>>>,
    clock: Box<dyn MonotonicClock>,
    ingress: Arc<Semaphore>,
    owner_nonce: u128,
    next_id: AtomicU64,
    wake: Notify,
    /// Becomes true when admission closes; long-lived streams end with it.
    closing: watch::Sender<bool>,
}

/// A newly admitted operation and the notifier its completion wakes.
struct Admission {
    id: String,
    notify: Arc<Notify>,
}

/// Holds one ingress permit for as long as a request body or its detached work is alive.
#[derive(Clone)]
pub(crate) struct IngressLease {
    _permit: Arc<OwnedSemaphorePermit>,
}

impl ControlOperations {
    /// Creates an open operation owner with a fresh session identifier namespace.
    /// Receipt expiry uses the supplied monotonic clock.
    #[must_use]
    pub fn new(clock: Box<dyn MonotonicClock>) -> Self {
        static OWNER_SEQUENCE: AtomicU64 = AtomicU64::new(1);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let sequence = OWNER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Self {
            inner: Arc::new(Inner {
                ledger: Mutex::new(ReceiptLedger::new(MAX_RECEIPTS, COMPLETED_TTL)),
                clock,
                ingress: Arc::new(Semaphore::new(INGRESS_PERMITS)),
                owner_nonce: now ^ (u128::from(sequence) << 64),
                next_id: AtomicU64::new(1),
                wake: Notify::new(),
                closing: watch::Sender::new(false),
            }),
            timings: OperationTimings::default(),
        }
    }

    /// Replaces the time bounds, for example short ones in a test harness.
    #[must_use]
    pub fn with_timings(mut self, timings: OperationTimings) -> Self {
        self.timings = timings;
        self
    }

    pub(crate) fn timings(&self) -> OperationTimings {
        self.timings
    }

    /// Stops accepting new work while allowing all admitted handlers to finish.
    pub fn close_admission(&self) {
        self.lock_ledger().close();
        self.inner.closing.send_replace(true);
    }

    /// Resolves once admission closes, so streamed responses let graceful shutdown finish.
    pub(crate) fn closed(&self) -> impl std::future::Future<Output = ()> + Send + use<> {
        let mut closing = self.inner.closing.subscribe();
        async move {
            let _ = closing.wait_for(|closing| *closing).await;
        }
    }

    /// Waits until every admitted handler has published a terminal receipt.
    pub async fn drain(&self) {
        loop {
            let notified = self.inner.wake.notified();
            if self.lock_ledger().pending() == 0 {
                return;
            }
            notified.await;
        }
    }

    pub(crate) fn try_acquire_ingress(&self) -> Option<IngressLease> {
        self.inner
            .ingress
            .clone()
            .try_acquire_owned()
            .ok()
            .map(|permit| IngressLease {
                _permit: Arc::new(permit),
            })
    }

    fn now(&self) -> mixrelay_time::Instant {
        self.inner.clock.now()
    }

    fn admit<'a>(
        &self,
        requested: Option<&'a str>,
    ) -> Result<Admission, Rejection<'a, Box<StoredResponse>>> {
        let notify = Arc::new(Notify::new());
        let now = self.now();
        let id = self
            .lock_ledger()
            .admit(requested, notify.clone(), now, || {
                let sequence = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
                format!("op-{:032x}-{:016x}", self.inner.owner_nonce, sequence)
            })?;
        Ok(Admission { id, notify })
    }

    fn complete(&self, id: &str, response: StoredResponse) {
        let now = self.now();
        let Some(notify) = self.lock_ledger().complete(id, Box::new(response), now) else {
            return;
        };
        notify.notify_waiters();
        self.inner.wake.notify_waiters();
    }

    fn snapshot(&self, id: &str) -> Snapshot<Box<StoredResponse>> {
        let now = self.now();
        self.lock_ledger().snapshot(id, now)
    }

    fn acknowledge(&self, id: &str) -> Ack {
        let now = self.now();
        self.lock_ledger().acknowledge(id, now)
    }

    fn take_completed(&self, id: &str) -> Option<StoredResponse> {
        self.lock_ledger()
            .take_completed(id)
            .map(|response| *response)
    }

    fn lock_ledger(&self) -> MutexGuard<'_, ReceiptLedger<Box<StoredResponse>, Arc<Notify>>> {
        self.inner
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
