//! Atomic map-mode capture of the latest eligible hardware control.
use crate::Kind;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const WAITING: u64 = 1;
const CAPTURED: u64 = 2;
const ACTIVE: u64 = WAITING | CAPTURED;
// Two state bits leave 62 bits for the selection epoch, independent of port identity.
const EPOCH: u64 = 4;
const EPOCH_MASK: u64 = !ACTIVE;
type CompareCapture<'a> = dyn FnMut(&AtomicU64, u64, u64) -> Result<u64, u64> + 'a;

struct Publication<'a>(&'a AtomicBool);

impl Drop for Publication<'_> {
    fn drop(&mut self) {
        // Release publishes the complete identity before another offer or reader owns it.
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Debug, Default)]
pub struct LearnCapture {
    state: AtomicU64,
    input: AtomicU64,
    publishing: AtomicBool,
}

impl LearnCapture {
    pub fn arm(&self) {
        self.select(WAITING);
    }
    pub fn cancel(&self) {
        self.select(0);
    }
    fn select(&self, state: u64) {
        // Selection changes run off the callback. Keep the epoch in the same CAS word
        // as the capture so an offer cannot cross cancellation or target replacement.
        let _ = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                Some((old.wrapping_add(EPOCH) & EPOCH_MASK) | state)
            });
    }
    #[must_use]
    pub fn active(&self) -> bool {
        self.state.load(Ordering::Acquire) & ACTIVE != 0
    }

    /// At most two CAS attempts; contention skips the offer without waiting.
    pub fn offer(&self, port: u32, bytes: &[u8]) {
        self.offer_with(port, bytes, &mut |input, previous, encoded| {
            input.compare_exchange(previous, encoded, Ordering::AcqRel, Ordering::Acquire)
        });
    }

    fn offer_with(&self, port: u32, bytes: &[u8], compare: &mut CompareCapture<'_>) {
        let previous = self.state.load(Ordering::Acquire);
        if previous & ACTIVE == 0 {
            return;
        }
        let [status, number, value] = bytes else {
            return;
        };
        if *number > 127
            || *value > 127
            || !matches!(status & 0xf0, 0x90 | 0xb0)
            || (status & 0xf0 == 0x90 && *value == 0)
        {
            return;
        }
        let encoded = (u64::from(port) << 12)
            | (u64::from(status & 15) << 7)
            | if status & 0xf0 == 0xb0 { 1 << 11 } else { 0 }
            | u64::from(*number);
        let Some(_publication) = self.publication() else {
            return;
        };
        // Readers and other offers skip this publication until its payload is complete.
        // arm/cancel never wait for it: changing the epoch defeats this capture CAS.
        // Advance on captures too, so an older offer cannot replace a newer capture
        // merely because both observations have the CAPTURED flag in this selection.
        let next = (previous.wrapping_add(EPOCH) & EPOCH_MASK) | CAPTURED;
        if compare(&self.state, previous, next).is_ok() {
            self.input.store(encoded, Ordering::Relaxed);
        }
    }

    fn publication(&self) -> Option<Publication<'_>> {
        // One attempt owns the identity. Acquire pairs with the previous owner's release;
        // failure reads no payload and returns immediately, including on the callback.
        self.publishing
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Publication(&self.publishing))
    }

    #[must_use]
    pub fn captured(&self) -> Option<(u32, u8, Kind, u8)> {
        let _publication = self.publication()?;
        if self.state.load(Ordering::Acquire) & CAPTURED == 0 {
            return None;
        }
        let encoded = self.input.load(Ordering::Relaxed);
        Some((
            u32::try_from((encoded >> 12) & 0xffff_ffff).unwrap_or_default(),
            u8::try_from((encoded >> 7) & 15).unwrap_or_default(),
            if encoded & (1 << 11) == 0 {
                Kind::Note
            } else {
                Kind::Cc
            },
            u8::try_from(encoded & 127).unwrap_or_default(),
        ))
    }
}

#[cfg(test)]
mod tests;
