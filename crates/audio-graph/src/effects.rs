//! MixRelay-authored built-in effect contracts. No third-party DSP runtime.
use serde::{Deserialize, Serialize};

pub const SIDECHAIN_PORT_ID: &str = "sidechain";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EffectConfig {
    pub version: u16,
    pub channels: u16,
    pub bypass: bool,
    pub effect: Effect,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "parameters",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Effect {
    Equalizer(EqualizerParameters),
    HighPass(FilterParameters),
    LowPass(FilterParameters),
    Gate(DynamicsParameters),
    Expander(DynamicsParameters),
    ChannelUtility(ChannelParameters),
    Pan(PositionParameters),
    Balance(PositionParameters),
    Delay(DelayParameters),
    Ducking(DuckingParameters),
    Reverb(ReverbParameters),
    Chorus(ChorusParameters),
}

macro_rules! parameters {
    ($name:ident { $($field:ident : $type:ty),+ $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $name { $(pub $field: $type),+ }
    };
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EqualizerParameters {
    pub low_db: f32,
    pub mid_db: f32,
    pub high_db: f32,
    pub low_frequency_hz: f32,
    pub low_q: f32,
    pub mid_frequency_hz: f32,
    pub mid_q: f32,
    pub high_frequency_hz: f32,
    pub high_q: f32,
}
parameters!(FilterParameters {
    frequency_hz: f32,
    q: f32
});
parameters!(DynamicsParameters {
    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    range_db: f32
});
/// An unsupported serialized effect mode.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unsupported {mode}: {value}")]
pub struct EffectModeError {
    pub mode: &'static str,
    pub value: u8,
}

macro_rules! numeric_mode {
    ($name:ident, $label:literal, {$($variant:ident = $value:literal),+ $(,)?}) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(try_from = "u8", into = "u8")]
        pub enum $name { $($variant = $value),+ }

        impl TryFrom<u8> for $name {
            type Error = EffectModeError;
            fn try_from(value: u8) -> Result<Self, Self::Error> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => Err(EffectModeError { mode: $label, value }),
                }
            }
        }
        impl From<$name> for u8 {
            fn from(value: $name) -> Self { value as Self }
        }
    };
}

numeric_mode!(ChannelMode, "channel mode", {
    Unchanged = 0, AverageMono = 1, LeftToBoth = 2, RightToBoth = 3, Swap = 4, InvertPolarity = 5,
});
numeric_mode!(DelayMode, "delay mode", { Independent = 0, PingPong = 1 });
numeric_mode!(ReverbAlgorithm, "reverb algorithm", { Classic = 0, Dense = 1 });

parameters!(ChannelParameters { mode: ChannelMode });
parameters!(PositionParameters { position: f32 });
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DelayParameters {
    pub time_ms: f32,
    pub feedback: f32,
    pub mix: f32,
    /// Independent echoes or alternating stereo-pair feedback; mono stays mono.
    pub mode: DelayMode,
}
parameters!(DuckingParameters {
    threshold_db: f32,
    reduction_db: f32,
    attack_ms: f32,
    release_ms: f32
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReverbParameters {
    pub decay_seconds: f32,
    pub damping: f32,
    pub mix: f32,
    pub room_size: f32,
    pub pre_delay_ms: f32,
    /// Classic four-comb/two-allpass or dense eight-comb/four-allpass topology.
    pub algorithm: ReverbAlgorithm,
}
parameters!(ChorusParameters {
    rate_hz: f32,
    depth_ms: f32,
    mix: f32
});

/// Invalid built-in effect configuration, independent of rendering or native capabilities.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EffectValidationError {
    #[error("effect version must be 1 and channels must be 1..64")]
    VersionOrChannels,
    #[error("pan requires one input channel and produces stereo")]
    PanChannels,
    #[error("balance/channel utility requires complete stereo pairs")]
    StereoPairs,
    #[error("{name} must be finite and within {minimum}..{maximum}")]
    ParameterRange {
        name: &'static str,
        minimum: f32,
        maximum: f32,
    },
}

impl EffectConfig {
    #[must_use]
    pub const fn output_channels(&self) -> u16 {
        if matches!(self.effect, Effect::Pan(_)) {
            2
        } else {
            self.channels
        }
    }

    /// # Errors
    /// Rejects unsupported versions, channel geometry and non-finite/out-of-range parameters.
    pub fn validate(&self) -> Result<(), EffectValidationError> {
        if self.version != 1 || !(1..=64).contains(&self.channels) {
            return Err(EffectValidationError::VersionOrChannels);
        }
        if matches!(self.effect, Effect::Pan(_)) && self.channels != 1 {
            return Err(EffectValidationError::PanChannels);
        }
        if matches!(self.effect, Effect::Balance(_) | Effect::ChannelUtility(_))
            && !self.channels.is_multiple_of(2)
        {
            return Err(EffectValidationError::StereoPairs);
        }
        match &self.effect {
            Effect::Equalizer(p) => {
                for value in [p.low_db, p.mid_db, p.high_db] {
                    range("EQ gain", value, -24.0, 24.0)?;
                }
                frequency(p.low_frequency_hz, p.low_q)?;
                frequency(p.mid_frequency_hz, p.mid_q)?;
                frequency(p.high_frequency_hz, p.high_q)
            }
            Effect::HighPass(p) | Effect::LowPass(p) => frequency(p.frequency_hz, p.q),
            Effect::Gate(p) | Effect::Expander(p) => {
                range("thresholdDb", p.threshold_db, -80.0, 0.0)?;
                range("ratio", p.ratio, 1.0, 20.0)?;
                range("rangeDb", p.range_db, 0.0, 80.0)?;
                times(p.attack_ms, p.release_ms)
            }
            Effect::ChannelUtility(_) => Ok(()),
            Effect::Pan(p) | Effect::Balance(p) => range("position", p.position, -1.0, 1.0),
            Effect::Delay(p) => {
                range("timeMs", p.time_ms, 0.0, 2_000.0)?;
                range("feedback", p.feedback, 0.0, 0.9)?;
                range("mix", p.mix, 0.0, 1.0)
            }
            Effect::Ducking(p) => {
                range("thresholdDb", p.threshold_db, -80.0, 0.0)?;
                range("reductionDb", p.reduction_db, 0.0, 60.0)?;
                times(p.attack_ms, p.release_ms)
            }
            Effect::Reverb(p) => {
                range("roomSize", p.room_size, 0.5, 2.0)?;
                range("preDelayMs", p.pre_delay_ms, 0.0, 200.0)?;
                range("decaySeconds", p.decay_seconds, 0.1, 6.0)?;
                range("damping", p.damping, 0.0, 0.95)?;
                range("mix", p.mix, 0.0, 1.0)
            }
            Effect::Chorus(p) => {
                range("rateHz", p.rate_hz, 0.05, 5.0)?;
                range("depthMs", p.depth_ms, 0.0, 10.0)?;
                range("mix", p.mix, 0.0, 1.0)
            }
        }
    }
}

fn frequency(hz: f32, q: f32) -> Result<(), EffectValidationError> {
    range("frequencyHz", hz, 20.0, 20_000.0)?;
    range("Q", q, 0.1, 10.0)
}
fn times(attack: f32, release: f32) -> Result<(), EffectValidationError> {
    range("attackMs", attack, 0.1, 200.0)?;
    range("releaseMs", release, 5.0, 2_000.0)
}
fn range(name: &'static str, value: f32, low: f32, high: f32) -> Result<(), EffectValidationError> {
    if value.is_finite() && (low..=high).contains(&value) {
        Ok(())
    } else {
        Err(EffectValidationError::ParameterRange {
            name,
            minimum: low,
            maximum: high,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Effect, EffectConfig};

    #[test]
    fn configs_missing_bypass_or_any_parameter_are_rejected() {
        let complete = r#"{"version":1,"channels":2,"bypass":false,"effect":{"kind":"delay","parameters":{"timeMs":100,"feedback":0.3,"mix":0.2,"mode":0}}}"#;
        serde_json::from_str::<EffectConfig>(complete).unwrap();
        for incomplete in [
            complete.replace(r#""bypass":false,"#, ""),
            complete.replace(r#","mode":0"#, ""),
            r#"{"version":1,"channels":2,"bypass":false,"effect":{"kind":"reverb","parameters":{"decaySeconds":1.2,"damping":0.5,"mix":0.2}}}"#.into(),
            r#"{"version":1,"channels":2,"bypass":false,"effect":{"kind":"equalizer","parameters":{"lowDb":2,"midDb":-3,"highDb":4,"midFrequencyHz":1200,"midQ":2}}}"#.into(),
        ] {
            assert!(
                serde_json::from_str::<EffectConfig>(&incomplete).is_err(),
                "{incomplete}"
            );
        }
    }

    #[test]
    fn spatial_geometry_and_delay_mode_reject_unsupported_values() {
        let delay: EffectConfig = serde_json::from_str(r#"{"version":1,"channels":2,"bypass":false,"effect":{"kind":"delay","parameters":{"timeMs":100,"feedback":0.3,"mix":0.2,"mode":0}}}"#).unwrap();
        let mut encoded = serde_json::to_value(delay).unwrap();
        for mode in [0, 1, 2, 255] {
            encoded["effect"]["parameters"]["mode"] = serde_json::json!(mode);
            let decoded = serde_json::from_value::<EffectConfig>(encoded.clone());
            assert_eq!(decoded.is_ok(), mode < 2);
            if let Ok(decoded) = decoded {
                assert!(decoded.validate().is_ok());
                assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
            }
        }
        let mut reverb: EffectConfig = serde_json::from_str(r#"{"version":1,"channels":2,"bypass":false,"effect":{"kind":"reverb","parameters":{"decaySeconds":1.2,"damping":0.5,"mix":0.2,"roomSize":1,"preDelayMs":0,"algorithm":0}}}"#).unwrap();
        let mut encoded = serde_json::to_value(&reverb).unwrap();
        for algorithm in [0, 1, 2, 255] {
            encoded["effect"]["parameters"]["algorithm"] = serde_json::json!(algorithm);
            let decoded = serde_json::from_value::<EffectConfig>(encoded.clone());
            assert_eq!(decoded.is_ok(), algorithm < 2);
            if let Ok(decoded) = decoded {
                assert!(decoded.validate().is_ok());
                assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
            }
        }
        for (size, pre, valid) in [
            (0.5, 0.0, true),
            (2.0, 200.0, true),
            (0.49, 0.0, false),
            (2.01, 0.0, false),
            (1.0, -0.1, false),
            (1.0, 200.1, false),
            (f32::NAN, 0.0, false),
            (1.0, f32::INFINITY, false),
        ] {
            let Effect::Reverb(p) = &mut reverb.effect else {
                unreachable!()
            };
            p.room_size = size;
            p.pre_delay_ms = pre;
            assert_eq!(reverb.validate().is_ok(), valid);
        }
    }

    #[test]
    fn all_equalizer_band_frequency_and_q_ranges_are_validated() {
        let original: EffectConfig = serde_json::from_str(r#"{"version":1,"channels":2,"bypass":false,"effect":{"kind":"equalizer","parameters":{"lowDb":0,"midDb":0,"highDb":0,"lowFrequencyHz":150,"lowQ":0.707,"midFrequencyHz":1000,"midQ":0.707,"highFrequencyHz":6000,"highQ":0.707}}}"#).unwrap();
        for band in 0..3 {
            for (hz, q, valid) in [
                (20.0, 0.1, true),
                (20_000.0, 10.0, true),
                (19.0, 1.0, false),
                (20_001.0, 1.0, false),
                (100.0, 0.099, false),
                (100.0, 10.001, false),
                (f32::NAN, 1.0, false),
                (100.0, f32::INFINITY, false),
            ] {
                let mut config = original.clone();
                let Effect::Equalizer(p) = &mut config.effect else {
                    unreachable!()
                };
                match band {
                    0 => {
                        p.low_frequency_hz = hz;
                        p.low_q = q;
                    }
                    1 => {
                        p.mid_frequency_hz = hz;
                        p.mid_q = q;
                    }
                    _ => {
                        p.high_frequency_hz = hz;
                        p.high_q = q;
                    }
                }
                assert_eq!(
                    config.validate().is_ok(),
                    valid,
                    "band {band}, {hz} Hz, Q {q}"
                );
            }
        }
    }

    #[test]
    fn channel_modes_preserve_numeric_documents_and_reject_unknown_values() {
        let mut encoded = serde_json::json!({
            "version": 1, "channels": 2, "bypass": false,
            "effect": {"kind": "channel_utility", "parameters": {"mode": 0}}
        });
        for mode in [0, 1, 2, 3, 4, 5, 6, 255] {
            encoded["effect"]["parameters"]["mode"] = serde_json::json!(mode);
            let decoded = serde_json::from_value::<EffectConfig>(encoded.clone());
            assert_eq!(decoded.is_ok(), mode < 6);
            if let Ok(decoded) = decoded {
                assert!(decoded.validate().is_ok());
                assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
            }
        }
    }
}
