//! Pick-up and per-target bounded-rate volume publication, driven by injected time.

use std::collections::BTreeMap;

use crate::{Binding, Mode, Target, mailbox::Input};

// 34ms gives a maximum of 30 gain publications in any one-second interval.
pub const GAIN_INTERVAL_MS: u64 = 34;

/// Volume state belongs to the binding and resets on reconnect or reassignment.
#[derive(Debug, Default)]
pub struct Pickup {
    previous: Option<f32>,
    acquired: bool,
    expected: Option<f32>,
    observed: Option<f32>,
}

impl Pickup {
    /// Returns no gain until the physical fader reaches/crosses the current level.
    /// A change by another control disarms pickup, including an editor/Stream Deck edit.
    pub fn map(&mut self, binding: &Binding, input: Input, current: f32) -> Option<f32> {
        if !current.is_finite() {
            return None;
        }
        let next = match binding.mode {
            Mode::Absolute => {
                let value = ((binding.min_db
                    + f32::from(input.value.min(127)) * (binding.max_db - binding.min_db) / 127.0)
                    * 2.0)
                    .round()
                    / 2.0;
                if self
                    .observed
                    .is_some_and(|observed| (observed - current).abs() > 0.01)
                    && self
                        .expected
                        .is_some_and(|expected| (expected - current).abs() > 0.01)
                {
                    self.acquired = false;
                    self.previous = None;
                }
                self.observed = Some(current);
                let reached = (value - current).abs() <= 0.5
                    || self
                        .previous
                        .is_some_and(|previous| (previous - current) * (value - current) <= 0.0);
                self.previous = Some(value);
                self.acquired |= reached;
                if !self.acquired {
                    self.expected = Some(current);
                    return None;
                }
                value
            }
            Mode::TwosComplement => input.relative_gain(false, binding.step_db, current)?,
            Mode::Offset64 => input.relative_gain(true, binding.step_db, current)?,
        }
        .clamp(-60.0, 12.0);
        self.expected = Some(next);
        Some(next)
    }
}

/// The editor's node gain slider is linear in dB with half-dB increments.
#[must_use]
pub fn absolute(value: u8) -> f32 {
    ((-60.0 + f32::from(value.min(127)) * 72.0 / 127.0) * 2.0).round() / 2.0
}

#[derive(Debug)]
struct Pending {
    value: f32,
    due: u64,
    dirty: bool,
}

/// Targets come from current bindings; latest absolute intent wins.
#[derive(Debug, Default)]
pub struct Coalescer {
    targets: BTreeMap<Target, Pending>,
}

impl Coalescer {
    pub fn push(&mut self, target: Target, value: f32) {
        let pending = self.targets.entry(target).or_insert(Pending {
            value,
            due: 0,
            dirty: false,
        });
        pending.value = value;
        pending.dirty = true;
    }

    /// Peek the most recent pending value when accumulating relative deltas.
    #[must_use]
    pub fn desired(&self, target: &Target) -> Option<f32> {
        self.targets
            .get(target)
            .filter(|entry| entry.dirty)
            .map(|entry| entry.value)
    }

    /// A dispatch attempt consumes intent even on failure; mutations are never replayed.
    pub fn take(&mut self, target: &Target, now_ms: u64) -> Option<f32> {
        let pending = self.targets.get_mut(target)?;
        if !pending.dirty || now_ms < pending.due {
            return None;
        }
        pending.dirty = false;
        pending.due = now_ms.saturating_add(GAIN_INTERVAL_MS);
        Some(pending.value)
    }

    /// Fence the next attempt from completion, including a slow control acknowledgement.
    pub fn complete(&mut self, target: &Target, now_ms: u64) {
        if let Some(pending) = self.targets.get_mut(target) {
            pending.due = now_ms.saturating_add(GAIN_INTERVAL_MS);
        }
    }

    /// Remove intent when a target/device disappears or bindings change.
    pub fn clear(&mut self) {
        self.targets.clear();
    }
}

#[cfg(test)]
mod tests;
