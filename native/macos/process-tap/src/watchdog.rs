//! Confirm a quiet/stalled tap with an independent, unmuted native observation.
//! Quiet PCM alone is never a reason to interrupt a live capture.

use super::{TapSession, native::Native};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

const QUIET_PERIOD: Duration = Duration::from_secs(3);
const PROBE_PERIOD: Duration = Duration::from_secs(1);
const RETRY_PERIOD: Duration = Duration::from_secs(10);
const QUIET_BUFFERS: u64 = 8;

/// Opaque native ownership, consumed by `retire_diagnostic` after removal from
/// the sole control slot. No audio/observer thread can obtain this handle.
#[derive(Debug)]
pub(super) struct Diagnostic {
    pub(super) session: *mut TapSession,
    pub(super) preparation_started: Instant,
    pub(super) preparation_time: Duration,
}

#[derive(Debug)]
struct Probe {
    native: Diagnostic,
    deadline: Instant,
}

#[derive(Debug, Default)]
pub(super) struct TapWatchdog {
    audible: u64,
    callbacks: u64,
    quiet_since: Option<Instant>,
    retry_at: Option<Instant>,
    probe: Option<Probe>,
}

impl TapWatchdog {
    pub(super) fn clear(&mut self, native: &impl Native) {
        if let Some(probe) = self.probe.take() {
            native.retire_diagnostic(probe.native);
        }
        self.quiet_since = None;
        self.retry_at = None;
    }

    fn observe(&mut self, session: &TapSession, now: Instant) {
        // Progress counters are independent atomic evidence, not a PCM snapshot.
        self.audible = session.audible_callbacks.load(Ordering::Relaxed);
        self.callbacks = session.callbacks.load(Ordering::Relaxed);
        self.quiet_since = Some(now);
    }

    /// Called only by the serialized, off-realtime format owner. A positive
    /// result means the independent tap heard PCM while this tap remained quiet.
    pub(super) fn poll(
        &mut self,
        native: &impl Native,
        session: &TapSession,
        now: Instant,
    ) -> bool {
        let audible = session.audible_callbacks.load(Ordering::Relaxed);
        if !session.running.load(Ordering::Acquire) || !session.delivering() {
            self.clear(native);
            return false;
        }
        if self.quiet_since.is_none() || audible != self.audible {
            self.clear(native);
            self.observe(session, now);
            return false;
        }
        if let Some(probe) = &self.probe {
            let observed = native.diagnostic_has_audio(&probe.native);
            // A callback can resume during the diagnostic query. Recheck before
            // deciding to retire a producer that has already recovered itself.
            let resumed = session.audible_callbacks.load(Ordering::Relaxed) != self.audible;
            if observed || resumed || now >= probe.deadline {
                self.clear(native);
                self.observe(session, now);
                self.retry_at = Some(now + RETRY_PERIOD);
                return observed && !resumed;
            }
            return false;
        }
        let quiet = self
            .quiet_since
            .is_some_and(|since| now.duration_since(since) >= QUIET_PERIOD);
        let callbacks = session
            .callbacks
            .load(Ordering::Relaxed)
            .wrapping_sub(self.callbacks);
        // No callbacks is also suspicious, including a successful native start
        // that never delivered its first block. Actual application silence still
        // produces no positive diagnostic evidence and therefore no restart.
        if !quiet
            || (callbacks != 0 && callbacks < QUIET_BUFFERS)
            || self.retry_at.is_some_and(|retry| now < retry)
        {
            return false;
        }
        self.retry_at = Some(now + RETRY_PERIOD);
        let selection = session.selection.borrow();
        let Some(selection) = selection.as_ref() else {
            return false;
        };
        let mut diagnostic_selection = selection.clone();
        diagnostic_selection.mute = false;
        if let Ok(diagnostic) = native.diagnostic(&diagnostic_selection, now) {
            // Native property queries and creation/start may take time. Start
            // the observation budget after successful preparation, using the
            // native boundary's explicit clock coordinates (also injectable).
            let deadline =
                diagnostic.preparation_started + diagnostic.preparation_time + PROBE_PERIOD;
            self.probe = Some(Probe {
                native: diagnostic,
                deadline,
            });
        }
        false
    }
}
