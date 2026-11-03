//! Prepared fractional delay history with bounded recursive samples.

use super::history_sample;
use crate::render::{RenderPlanError, buffers};

#[derive(Debug)]
pub(super) struct Line {
    samples: Box<[f32]>,
    cursor: usize,
    valid: usize,
    pub(super) damped: f32,
}
impl Line {
    pub(super) fn new(frames: usize) -> Result<Self, RenderPlanError> {
        let samples = buffers::filled(frames, 0.0)?;
        Ok(Self {
            samples,
            cursor: 0,
            valid: 0,
            damped: 0.0,
        })
    }
    pub(super) fn push(&mut self, value: f32) {
        self.samples[self.cursor] = history_sample(value);
        self.cursor = (self.cursor + 1) % self.samples.len();
        self.valid = (self.valid + 1).min(self.samples.len());
    }
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )] // Delay clamped to a prepared line's capacity.
    pub(super) fn read(&self, delay: f32) -> f32 {
        let delay = delay.clamp(0.0, (self.samples.len() - 2) as f32);
        let whole = delay.floor() as usize;
        let fraction = delay - whole as f32;
        let at = |age: usize| {
            if age < self.valid {
                self.samples[(self.cursor + self.samples.len() - 1 - age) % self.samples.len()]
            } else {
                0.0
            }
        };
        at(whole).mul_add(1.0 - fraction, at(whole + 1) * fraction)
    }
    pub(super) fn reset(&mut self) {
        self.cursor = 0;
        self.valid = 0;
        self.damped = 0.0;
    }
}
