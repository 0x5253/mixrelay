//! Source-time coordinates follow the sinc interpolation position, not callback arrival.
#![expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "stream positions interpolate in f64; ring slots are bounded by prepared capacity"
)]
use super::{BridgeConfig, BridgeError};

/// The prepared sinc length is also the size of its retained timestamp history.
pub(super) const SINC_FRAMES: usize = 256;

pub(super) struct ConverterTime {
    history: Vec<u64>,
    written: u64,
    consumed: u64,
    position: f64,
    ratio: f64,
    target_ratio: f64,
    nominal_ratio: f64,
    sinc: bool,
}

impl ConverterTime {
    pub(super) fn new(config: BridgeConfig, sinc: bool) -> Result<Self, BridgeError> {
        let capacity = config.capacity_frames + 2 * SINC_FRAMES;
        let mut history = Vec::new();
        history
            .try_reserve_exact(capacity)
            .map_err(|_| BridgeError::Allocation { samples: capacity })?;
        history.resize(capacity, 0);
        let ratio = f64::from(config.output_rate) / f64::from(config.input_rate);
        Ok(Self {
            history,
            written: 0,
            consumed: 0,
            position: 0.0,
            ratio,
            target_ratio: ratio,
            nominal_ratio: ratio,
            sinc,
        })
    }

    pub(super) fn push(&mut self, frames: usize, times: Option<&[u64]>) {
        for frame in 0..frames {
            let slot = self.written as usize % self.history.len();
            self.history[slot] = times.map_or(0, |times| times[frame]);
            self.written += 1;
        }
    }

    pub(super) fn ratio(&mut self, correction: f64) {
        self.target_ratio = self.nominal_ratio * (1.0 + correction * 1e-6);
    }

    pub(super) fn reset(&mut self) {
        self.written = 0;
        self.consumed = 0;
        // Rubato 5's sinc starts at -(length-1), with its kernel centred
        // length/2 frames later. The increment precedes each output sample.
        self.position = if self.sinc {
            -(SINC_FRAMES as f64) / 2.0
        } else {
            0.0
        };
        self.ratio = self.nominal_ratio;
        self.target_ratio = self.nominal_ratio;
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "bounded converter batch and stream frame positions"
    )]
    pub(super) fn convert(&mut self, consumed: usize, output: &mut [u64]) {
        if self.sinc {
            let mut step = 1.0 / self.ratio;
            let increment = if output.is_empty() {
                0.0
            } else {
                (1.0 / self.target_ratio - step) / output.len() as f64
            };
            for time in output {
                step += increment;
                self.position += step;
                *time = self.at(self.position);
            }
            self.ratio = self.target_ratio;
        } else {
            for (index, time) in output.iter_mut().enumerate() {
                *time = self.at((self.consumed + index as u64) as f64);
            }
        }
        self.consumed += consumed as u64;
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "finite validated source coordinate, interpolation bounded by its two timestamps"
    )]
    fn at(&self, position: f64) -> u64 {
        if !position.is_finite() || position < 0.0 || position >= self.written as f64 {
            return 0;
        }
        let index = position.floor() as u64;
        if self.written - index > self.history.len() as u64 {
            return 0;
        }
        let first = self.history[index as usize % self.history.len()];
        let fraction = position.fract();
        if first == 0 || fraction == 0.0 {
            return first;
        }
        if index + 1 >= self.written {
            return 0;
        }
        let second = self.history[(index + 1) as usize % self.history.len()];
        if second <= first {
            return 0;
        }
        first.saturating_add(((second - first) as f64 * fraction).round() as u64)
    }
}

pub(super) fn zeroed(frames: usize) -> Result<Vec<u64>, BridgeError> {
    let mut times = Vec::new();
    times
        .try_reserve_exact(frames)
        .map_err(|_| BridgeError::Allocation { samples: frames })?;
    times.resize(frames, 0);
    Ok(times)
}
