//! Control-thread coefficient and bounded delay-storage design.

use super::filters::{Shape, coefficients};
use super::{Design, Effect, EffectConfig, MAX_EFFECT_BYTES, REVERB_DELAYS, db, smoothing};
use crate::render::RenderFormat;

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)] // Validated rate and bounded delay calculations.
impl Design {
    #[allow(clippy::too_many_lines)] // One bounded, exhaustive preparation table for the effect contract.
    pub(in crate::render) fn new(
        config: &EffectConfig,
        format: RenderFormat,
    ) -> Result<Self, String> {
        config.validate().map_err(|error| error.to_string())?;
        let rate = format.sample_rate as f32;
        if format.sample_rate == 0 {
            return Err("effect sample rate must be positive".into());
        }
        let mut design = Self {
            config: config.clone(),
            target: [0.0; 32],
            filter_target: [0.0; 15],
            filters: 0,
            line_frames: vec![],
            smoothing: 1.0 - (-1.0 / (rate * 0.005)).exp(),
        };
        let t = &mut design.target;
        match &config.effect {
            Effect::Equalizer(p) => {
                design.filters = 3;
                for (index, (shape, hz, q, db)) in [
                    (Shape::Peak, p.low_frequency_hz, p.low_q, p.low_db),
                    (Shape::Peak, p.mid_frequency_hz, p.mid_q, p.mid_db),
                    (Shape::Peak, p.high_frequency_hz, p.high_q, p.high_db),
                ]
                .into_iter()
                .enumerate()
                {
                    design.filter_target[index * 5..index * 5 + 5].copy_from_slice(&coefficients(
                        shape,
                        hz,
                        q,
                        db,
                        format.sample_rate,
                    ));
                }
            }
            Effect::HighPass(p) | Effect::LowPass(p) => {
                design.filters = 1;
                let shape = if matches!(config.effect, Effect::HighPass(_)) {
                    Shape::HighPass
                } else {
                    Shape::LowPass
                };
                design.filter_target[..5].copy_from_slice(&coefficients(
                    shape,
                    p.frequency_hz,
                    p.q,
                    0.0,
                    format.sample_rate,
                ));
            }
            Effect::Gate(p) | Effect::Expander(p) => {
                t[0] = db(p.threshold_db);
                t[1] = p.ratio - 1.0;
                t[2] = smoothing(p.attack_ms, rate);
                t[3] = smoothing(p.release_ms, rate);
                t[4] = db(-p.range_db);
            }
            Effect::ChannelUtility(_) => {}
            Effect::Pan(p) => {
                let angle = (p.position + 1.0) * std::f32::consts::FRAC_PI_4;
                t[0] = angle.cos();
                t[1] = angle.sin();
            }
            Effect::Balance(p) => {
                t[0] = 1.0 - p.position.max(0.0);
                t[1] = 1.0 + p.position.min(0.0);
            }
            Effect::Delay(p) => {
                t[0] = p.time_ms * rate / 1_000.0;
                t[1] = p.feedback;
                t[2] = p.mix;
                t[3] = f32::from(u8::from(p.mode));
                design.line_frames =
                    vec![(2.0 * rate).ceil() as usize + 2; usize::from(config.channels)];
            }
            Effect::Ducking(p) => {
                t[0] = db(p.threshold_db);
                t[1] = db(-p.reduction_db);
                t[2] = smoothing(p.attack_ms, rate);
                t[3] = smoothing(p.release_ms, rate);
            }
            Effect::Reverb(p) => {
                t[8] = p.damping;
                t[9] = p.mix;
                t[10] = p.pre_delay_ms * rate / 1_000.0;
                t[11] = f32::from(u8::from(p.algorithm));
                t[24] = (0.0007 * rate * p.room_size).round();
                for (index, seconds) in REVERB_DELAYS.into_iter().enumerate() {
                    let duration = seconds * p.room_size;
                    t[12 + index] = (duration * rate).round().max(1.0);
                    if index < 8 {
                        t[index] = (-6.907_755 * duration / p.decay_seconds).exp();
                    }
                }
                for channel in 0..config.channels {
                    // Reserve maximum room size and pre-delay once, so preset/size
                    // changes can reuse the existing state without callback allocation.
                    design.line_frames.push((0.2 * rate).ceil() as usize + 2);
                    for seconds in REVERB_DELAYS {
                        let duration = seconds + f32::from(channel % 2) * 0.0007;
                        design
                            .line_frames
                            .push((duration * 2.0 * rate).ceil() as usize + 2);
                    }
                }
            }
            Effect::Chorus(p) => {
                t[0] = p.rate_hz / rate;
                t[1] = p.depth_ms * rate / 1_000.0;
                t[2] = p.mix;
                t[3] = 0.015 * rate;
                design.line_frames =
                    vec![(0.05 * rate).ceil() as usize + 2; usize::from(config.channels)];
            }
        }
        t[31] = if config.bypass { 0.0 } else { 1.0 };
        if design.storage_bytes()? > MAX_EFFECT_BYTES {
            return Err("built-in effect memory capacity exceeded".into());
        }
        Ok(design)
    }

    pub(in crate::render) fn storage_bytes(&self) -> Result<usize, String> {
        self.line_frames.iter().try_fold(0_usize, |sum, frames| {
            frames
                .checked_mul(size_of::<f32>())
                .and_then(|bytes| sum.checked_add(bytes))
                .ok_or_else(|| "effect buffer size overflow".into())
        })
    }

    pub(in crate::render) fn compatible(&self, other: &Self) -> bool {
        self.config.channels == other.config.channels
            && self.line_frames == other.line_frames
            && std::mem::discriminant(&self.config.effect)
                == std::mem::discriminant(&other.config.effect)
    }
}
