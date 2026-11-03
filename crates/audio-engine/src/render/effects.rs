//! Prepared, bounded, MixRelay-authored DSP. Allocation and destruction belong to control.
mod delay_line;
mod design;
mod filters;
use super::{PortSlot, RenderPlanError, buffers};
use crate::dsp::db_to_linear as db;
use delay_line::Line;
use mixrelay_audio_graph::{ChannelMode, Effect, EffectConfig, NodeId};

pub(super) const MAX_EFFECT_BYTES: usize = 128 * 1024 * 1024;
// MixRelay tuning, not copied from Freeverb's tuning.h. Eight lowpass feedback
// combs followed by four genuine Schroeder allpasses; see spatial-effect design notes.
const REVERB_DELAYS: [f32; 12] = [
    0.0297, 0.0371, 0.0411, 0.0437, 0.0313, 0.0337, 0.0397, 0.0473, 0.005, 0.0017, 0.0061, 0.0031,
];
const REVERB_LINES: usize = 13;

#[derive(Debug)]
pub(super) struct Binding {
    pub(super) node_id: NodeId,
    pub(super) design: Design,
}

#[derive(Debug, Clone)]
pub(super) struct Design {
    config: EffectConfig,
    target: [f32; 32],
    filter_target: [f64; 15],
    filters: usize,
    line_frames: Vec<usize>,
    smoothing: f32,
}

#[derive(Debug)]
pub(super) struct State {
    current: [f32; 32],
    filter_current: [f64; 15],
    histories: Box<[[f64; 2]]>,
    lines: Vec<Line>,
    envelope: f32,
    gain: f32,
    phase: f32,
    suspended: bool,
    mode: ChannelMode,
    previous_mode: ChannelMode,
    mode_mix: f32,
}
#[allow(clippy::too_many_arguments)] // Explicit prepared port geometry and arena; no runtime wrapper allocation.
impl State {
    pub(super) fn new(design: &Design) -> Result<Self, RenderPlanError> {
        let mut lines = buffers::reserve(design.line_frames.len())?;
        for &frames in &design.line_frames {
            lines.push(Line::new(frames)?);
        }
        let mut histories = buffers::reserve(design.filters * usize::from(design.config.channels))?;
        histories.resize(
            design.filters * usize::from(design.config.channels),
            [0.0; 2],
        );
        let mode = if let Effect::ChannelUtility(p) = &design.config.effect {
            p.mode
        } else {
            ChannelMode::Unchanged
        };
        Ok(Self {
            current: design.target,
            filter_current: design.filter_target,
            histories: histories.into_boxed_slice(),
            lines,
            envelope: 0.0,
            gain: 1.0,
            phase: 0.0,
            suspended: false,
            mode,
            previous_mode: mode,
            mode_mix: 1.0,
        })
    }
    pub(super) fn suspend(&mut self) {
        if self.suspended {
            return;
        }
        self.histories.fill([0.0; 2]);
        for line in &mut self.lines {
            line.reset();
        }
        self.envelope = 0.0;
        self.gain = 1.0;
        self.suspended = true;
    }
    pub(super) fn process(
        &mut self,
        design: &Design,
        arena: &mut [f32],
        input: PortSlot,
        output: PortSlot,
        sidechain: Option<PortSlot>,
        frames: usize,
    ) {
        self.suspended = false;
        let channels = usize::from(input.channels);
        let outputs = usize::from(output.channels);
        if let Effect::ChannelUtility(p) = &design.config.effect
            && p.mode != self.mode
        {
            self.previous_mode = self.mode;
            self.mode = p.mode;
            self.mode_mix = 0.0;
        }
        for frame in 0..frames {
            for (current, target) in self.current.iter_mut().zip(design.target) {
                let next = *current + design.smoothing * (target - *current);
                *current = if next.to_bits() == current.to_bits() || (next - target).abs() < 1e-7 {
                    target
                } else {
                    next
                };
            }
            for (current, target) in self.filter_current.iter_mut().zip(design.filter_target) {
                let next = *current + f64::from(design.smoothing) * (target - *current);
                *current = if next.to_bits() == current.to_bits() {
                    target
                } else {
                    next
                };
            }
            let start = input.offset + frame * channels;
            let end = output.offset + frame * outputs;
            self.update_gain(
                design,
                arena,
                start,
                channels,
                sidechain.map(|port| arena[port.offset + frame]),
            );
            self.mode_mix = (self.mode_mix + design.smoothing).min(1.0);
            if matches!(design.config.effect, Effect::Delay(_)) {
                self.delay_frame(arena, start, end, outputs);
                continue;
            }
            for channel in 0..outputs {
                let source = finite(arena[start + channel.min(channels - 1)]);
                let processed = self.sample(design, arena, start, channel, source);
                arena[end + channel] = if self.current[31] == 0.0 {
                    source
                } else {
                    finite(source + self.current[31] * (processed - source))
                };
            }
            if matches!(design.config.effect, Effect::Chorus(_)) {
                self.phase = (self.phase + self.current[0]).fract();
            }
        }
    }

    fn delay_frame(&mut self, arena: &mut [f32], start: usize, end: usize, channels: usize) {
        let c = &self.current;
        let mut echoes = [0.0_f32; 64];
        for (channel, echo) in echoes[..channels].iter_mut().enumerate() {
            *echo = self.lines[channel].read((c[0] - 1.0).max(0.0));
        }
        for channel in 0..channels {
            let source = finite(arena[start + channel]);
            let partner = channel ^ 1;
            let cross = if partner < channels { c[3] } else { 0.0 };
            let drive = if channel.is_multiple_of(2) && partner < channels {
                (source + finite(arena[start + partner])) * 0.5
            } else {
                0.0
            };
            let injected = source + cross * (drive - source);
            let feedback =
                echoes[channel] + cross * (echoes[partner.min(channels - 1)] - echoes[channel]);
            let line = &mut self.lines[channel];
            // Zero/sub-sample time never creates an instantaneous feedback loop.
            line.push(if c[0] >= 1.0 {
                injected + feedback * c[1]
            } else {
                source
            });
            let wet = line.read(c[0]);
            let processed = source + c[2] * (wet - source);
            arena[end + channel] = if c[31] == 0.0 {
                source
            } else {
                finite(source + c[31] * (processed - source))
            };
        }
    }

    fn update_gain(
        &mut self,
        design: &Design,
        arena: &[f32],
        start: usize,
        channels: usize,
        control: Option<f32>,
    ) {
        let c = &self.current;
        let target = match &design.config.effect {
            Effect::Gate(_) | Effect::Expander(_) => {
                let peak = arena[start..start + channels]
                    .iter()
                    .fold(0.0_f32, |p, x| p.max(finite(*x).abs()));
                let smoothing = if peak > self.envelope { c[2] } else { c[3] };
                self.envelope = smoothing.mul_add(self.envelope, (1.0 - smoothing) * peak);
                if self.envelope >= c[0] {
                    1.0
                } else if matches!(design.config.effect, Effect::Gate(_)) {
                    c[4]
                } else {
                    (self.envelope / c[0]).powf(c[1]).max(c[4])
                }
            }
            Effect::Ducking(_) => {
                if finite(control.unwrap_or(0.0)).abs() >= c[0] {
                    c[1]
                } else {
                    1.0
                }
            }
            _ => return,
        };
        // Gate opens quickly; ducking closes quickly. Both linked channels use one gain.
        let attack = matches!(design.config.effect, Effect::Ducking(_)) == (target < self.gain);
        let smoothing = if attack { c[2] } else { c[3] };
        self.gain = smoothing.mul_add(self.gain, (1.0 - smoothing) * target);
    }

    fn sample(
        &mut self,
        design: &Design,
        arena: &[f32],
        start: usize,
        channel: usize,
        source: f32,
    ) -> f32 {
        let c = &self.current;
        match &design.config.effect {
            Effect::Equalizer(_) | Effect::HighPass(_) | Effect::LowPass(_) => {
                let mut result = source;
                for stage in 0..design.filters {
                    result = filters::sample(
                        result,
                        &self.filter_current[stage * 5..stage * 5 + 5],
                        &mut self.histories[channel * design.filters + stage],
                    );
                }
                result
            }
            Effect::Gate(_) | Effect::Expander(_) | Effect::Ducking(_) => source * self.gain,
            Effect::Pan(_) | Effect::Balance(_) => source * c[channel % 2],
            Effect::ChannelUtility(_) => {
                let pair = start + channel / 2 * 2;
                let left = finite(arena[pair]);
                let right = finite(arena[pair + 1]);
                let old = channel_sample(self.previous_mode, channel, left, right);
                let new = channel_sample(self.mode, channel, left, right);
                old + self.mode_mix * (new - old)
            }
            Effect::Delay(_) => unreachable!("delay pairs are processed atomically per frame"),
            Effect::Chorus(_) => {
                let line = &mut self.lines[channel];
                line.push(source);
                #[allow(clippy::cast_precision_loss)]
                let phase = self.phase + (channel % 2) as f32 * 0.25;
                let modulation = (phase * std::f32::consts::TAU).sin();
                let wet = line.read(c[3] + c[1] * modulation);
                source + c[2] * (wet - source)
            }
            Effect::Reverb(_) => {
                let lines = &mut self.lines[channel * REVERB_LINES..(channel + 1) * REVERB_LINES];
                let partner = channel ^ 1;
                let excitation_source = if partner < usize::from(design.config.channels) {
                    (source + finite(arena[start + partner])) * 0.5
                } else {
                    source
                };
                lines[0].push(source + c[11] * (excitation_source - source));
                let excitation = lines[0].read(c[10]);
                #[allow(clippy::cast_precision_loss)]
                let spread = (channel % 2) as f32 * c[24];
                let mut classic = 0.0;
                let mut dense = 0.0;
                for (index, line) in lines[1..9].iter_mut().enumerate() {
                    let delayed = line.read(c[12 + index] + spread - 1.0);
                    line.damped = history_sample(delayed * (1.0 - c[8]) + line.damped * c[8]);
                    line.push(excitation + line.damped * c[index]);
                    if index < 4 {
                        classic += delayed * 0.25;
                    }
                    dense += delayed * 0.125;
                }
                let mut wet = classic + c[11] * (dense - classic);
                let mut classic_diffusion = 0.0;
                for (index, line) in lines[9..].iter_mut().enumerate() {
                    let delayed = line.read(c[20 + index] + spread - 1.0);
                    let result = delayed - 0.5 * wet;
                    line.push(wet + 0.5 * result);
                    wet = result;
                    if index == 1 {
                        classic_diffusion = wet;
                    }
                }
                wet = classic_diffusion + c[11] * (wet - classic_diffusion);
                source + c[9] * (wet - source)
            }
        }
    }
}

fn channel_sample(mode: ChannelMode, channel: usize, left: f32, right: f32) -> f32 {
    match mode {
        ChannelMode::AverageMono => (left + right) * 0.5,
        ChannelMode::LeftToBoth => left,
        ChannelMode::RightToBoth => right,
        ChannelMode::Swap => {
            if channel.is_multiple_of(2) {
                right
            } else {
                left
            }
        }
        ChannelMode::InvertPolarity => {
            -if channel.is_multiple_of(2) {
                left
            } else {
                right
            }
        }
        ChannelMode::Unchanged => {
            if channel.is_multiple_of(2) {
                left
            } else {
                right
            }
        }
    }
}
fn smoothing(ms: f32, rate: f32) -> f32 {
    crate::dsp::smoothing_coefficient(ms * rate / 1_000.0)
}
fn finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

// Flush inaudible recursive history before denormals increase callback CPU cost.
// Dry/bypassed input retains its finite sample values unchanged.
fn history_sample(value: f32) -> f32 {
    if value.abs() < 1e-20 {
        0.0
    } else {
        finite(value)
    }
}
