//! Operation receipts: admission, deduplication, completion, acknowledgement and expiry.
//!
//! Callers pass the current time and new identities in, so every transition can be
//! tested without a runtime or real time. `R` is the stored response and `W` whatever
//! the caller wakes when an operation completes.
use std::{collections::HashMap, time::Duration};

use mixrelay_time::Instant;

pub(super) struct ReceiptLedger<R, W> {
    capacity: usize,
    completed_ttl: Duration,
    admission_open: bool,
    pending: usize,
    receipts: HashMap<String, Receipt<R, W>>,
}

enum Receipt<R, W> {
    /// Running; `W` is woken when it completes.
    Pending(W),
    Complete {
        response: R,
        completed_at: Instant,
    },
}

/// What a receipt says about an operation.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Snapshot<R> {
    Pending,
    Complete(R),
    Missing,
}

/// Why `admit` did not start a new operation.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Rejection<'a, R> {
    /// The client ID already names a running operation.
    Pending(&'a str),
    /// The client ID already names a completed operation; replay its response.
    Complete(&'a str, R),
    /// Admission is closed or every receipt is in use.
    Full,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Ack {
    Completed,
    Pending,
    Missing,
}

impl<R: Clone, W> ReceiptLedger<R, W> {
    pub(super) fn new(capacity: usize, completed_ttl: Duration) -> Self {
        Self {
            capacity,
            completed_ttl,
            admission_open: true,
            pending: 0,
            receipts: HashMap::new(),
        }
    }

    /// Operations admitted and not yet complete.
    pub(super) const fn pending(&self) -> usize {
        self.pending
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.receipts.len()
    }

    /// Stops new admissions. Known client IDs still resolve to their receipts.
    pub(super) fn close(&mut self) {
        self.admission_open = false;
    }

    /// Starts an operation under the client's ID, or under the first identity from
    /// `issue` that no receipt uses, and keeps `waiter` until it completes.
    ///
    /// Expired receipts are dropped first, so their IDs start new work. `now` may be read
    /// slightly before another caller's: expiry saturates, so a late reading never expires
    /// a receipt early.
    ///
    /// # Errors
    ///
    /// Checked in this order: a client ID with a kept receipt returns that receipt
    /// (`Pending` or `Complete`) even while admission is closed; then `Full` when
    /// admission is closed or every receipt is in use.
    pub(super) fn admit<'a>(
        &mut self,
        requested: Option<&'a str>,
        waiter: W,
        now: Instant,
        mut issue: impl FnMut() -> String,
    ) -> Result<String, Rejection<'a, R>> {
        self.purge_expired(now);
        if let Some(id) = requested
            && let Some(receipt) = self.receipts.get(id)
        {
            return Err(match receipt {
                Receipt::Pending(_) => Rejection::Pending(id),
                Receipt::Complete { response, .. } => Rejection::Complete(id, response.clone()),
            });
        }
        if !self.admission_open || self.receipts.len() >= self.capacity {
            return Err(Rejection::Full);
        }
        let id = requested.map_or_else(
            || loop {
                let id = issue();
                if !self.receipts.contains_key(&id) {
                    break id;
                }
            },
            str::to_owned,
        );
        self.receipts.insert(id.clone(), Receipt::Pending(waiter));
        self.pending += 1;
        Ok(id)
    }

    /// Records the response of a pending operation and returns its waiter to wake.
    /// A missing or already completed receipt is left unchanged.
    pub(super) fn complete(&mut self, id: &str, response: R, now: Instant) -> Option<W> {
        let receipt = self.receipts.get_mut(id)?;
        let completed = Receipt::Complete {
            response,
            completed_at: now,
        };
        match std::mem::replace(receipt, completed) {
            Receipt::Pending(waiter) => {
                self.pending = self.pending.saturating_sub(1);
                Some(waiter)
            }
            // The first completion stands.
            first @ Receipt::Complete { .. } => {
                *receipt = first;
                None
            }
        }
    }

    pub(super) fn snapshot(&mut self, id: &str, now: Instant) -> Snapshot<R> {
        self.purge_expired(now);
        match self.receipts.get(id) {
            Some(Receipt::Pending(_)) => Snapshot::Pending,
            Some(Receipt::Complete { response, .. }) => Snapshot::Complete(response.clone()),
            None => Snapshot::Missing,
        }
    }

    /// Removes a completed receipt so its ID and slot can be reused.
    pub(super) fn acknowledge(&mut self, id: &str, now: Instant) -> Ack {
        self.purge_expired(now);
        match self.receipts.get(id) {
            None => Ack::Missing,
            Some(Receipt::Pending(_)) => Ack::Pending,
            Some(Receipt::Complete { .. }) => {
                self.receipts.remove(id);
                Ack::Completed
            }
        }
    }

    /// Removes and returns a completed response that no client will poll for.
    pub(super) fn take_completed(&mut self, id: &str) -> Option<R> {
        let (id, receipt) = self.receipts.remove_entry(id)?;
        match receipt {
            Receipt::Complete { response, .. } => Some(response),
            pending @ Receipt::Pending(_) => {
                self.receipts.insert(id, pending);
                None
            }
        }
    }

    /// Completed receipts expire `completed_ttl` after completion; pending ones never do.
    fn purge_expired(&mut self, now: Instant) {
        let ttl = self.completed_ttl;
        self.receipts.retain(|_, receipt| {
            !matches!(receipt, Receipt::Complete { completed_at, .. }
                if now.duration_since(*completed_at) >= ttl)
        });
    }
}

#[cfg(test)]
mod tests;
