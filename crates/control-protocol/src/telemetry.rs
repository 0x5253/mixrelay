//! Multiplexed control observations and packed meter transport.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

use crate::{
    AppStatus, DynamicsMeterSnapshot, EngineControlSnapshot, GraphRuntimeStatus, MeterSnapshot,
    NodeMeterSnapshot, ProcessorEditorStatus,
};

/// A complete replacement, including absence of controls and removed editors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlStateSnapshot {
    pub generation: u64,
    pub status: AppStatus,
    pub graph: GraphRuntimeStatus,
    pub controls: Option<EngineControlSnapshot>,
    pub editors: Vec<ProcessorEditorStatus>,
}

/// One multiplexed publication. State is optional only after the initial snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlTelemetry {
    pub engine_incarnation: String,
    pub sequence: u64,
    #[serde(with = "packed")]
    pub meters: MeterSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ControlStateSnapshot>,
}

/// Encodes one ordered stream. Meter bindings are sent only when they differ from the
/// bindings this stream last carried; every hop is ordered and lossless, and coalescing
/// happens before encoding, so a decoder always holds the bindings that apply.
#[derive(Debug, Default)]
pub struct TelemetryEncoder {
    sent: Option<Vec<Binding>>,
}

impl TelemetryEncoder {
    /// The JSON form, with base64 samples.
    #[must_use]
    pub fn json(&mut self, packet: &ControlTelemetry) -> serde_json::Value {
        let (meters, _) = self.pack(&packet.meters);
        Self::metadata(packet, &meters)
    }

    /// A raw webview frame: LE u32 JSON byte length, the JSON padded to four bytes, then
    /// the LE f32 samples, so the webview can read them as a `Float32Array` in place.
    #[must_use]
    pub fn raw(&mut self, packet: &ControlTelemetry) -> Vec<u8> {
        let (mut meters, samples) = self.pack(&packet.meters);
        meters.samples.clear();
        let json = Self::metadata(packet, &meters).to_string().into_bytes();
        let mut frame = Vec::with_capacity(json.len() + samples.len() + 8);
        // The bounded control transport rejects frames long before u32's limit.
        frame.extend_from_slice(&json.len().to_le_bytes()[..4]);
        frame.extend_from_slice(&json);
        frame.resize((frame.len() + 3) & !3, 0);
        frame.extend(samples);
        frame
    }

    fn pack(&mut self, meters: &MeterSnapshot) -> (PackedMeters, Vec<u8>) {
        let (mut packed, samples) = PackedMeters::pack(meters);
        if self.sent == packed.bindings {
            packed.bindings = None;
        } else {
            self.sent.clone_from(&packed.bindings);
        }
        (packed, samples)
    }

    fn metadata(packet: &ControlTelemetry, meters: &PackedMeters) -> serde_json::Value {
        let mut value = serde_json::json!({"engineIncarnation": packet.engine_incarnation,
            "sequence": packet.sequence, "meters": meters});
        if let Some(state) = &packet.state {
            value["state"] = serde_json::json!(state);
        }
        value
    }
}

/// A malformed packed observation, preserving the codec failure at its boundary.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryDecodeError {
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Base64(#[from] base64::DecodeError),
    #[error("meter bindings missing")]
    MissingBindings,
    #[error("invalid packed meter length")]
    InvalidLength,
}

/// Decodes one ordered stream written by a [`TelemetryEncoder`] (or full packets).
#[derive(Debug, Default)]
pub struct TelemetryDecoder {
    bindings: Option<Vec<Binding>>,
}

impl TelemetryDecoder {
    /// # Errors
    /// Malformed JSON or samples, or omitted bindings before any were received.
    pub fn json(
        &mut self,
        value: serde_json::Value,
    ) -> Result<ControlTelemetry, TelemetryDecodeError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            engine_incarnation: String,
            sequence: u64,
            meters: PackedMeters,
            #[serde(default)]
            state: Option<ControlStateSnapshot>,
        }
        let mut wire: Wire = serde_json::from_value(value)?;
        match wire.meters.bindings.take() {
            Some(bindings) => self.bindings = Some(bindings),
            None if self.bindings.is_none() => return Err(TelemetryDecodeError::MissingBindings),
            None => {}
        }
        wire.meters.bindings.clone_from(&self.bindings);
        Ok(ControlTelemetry {
            engine_incarnation: wire.engine_incarnation,
            sequence: wire.sequence,
            meters: wire.meters.unpack()?,
            state: wire.state,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Binding {
    node_id: String,
    port_id: String,
    channels: usize,
    input_channels: Option<usize>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackedMeters {
    protocol_version: u16,
    revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bindings: Option<Vec<Binding>>,
    samples: String,
}

impl PackedMeters {
    fn pack(meters: &MeterSnapshot) -> (Self, Vec<u8>) {
        let mut samples = Vec::new();
        let bindings: Vec<Binding> = meters
            .nodes
            .iter()
            .map(|node| {
                for value in &node.peaks {
                    samples.extend(value.to_le_bytes());
                }
                if let Some(dynamics) = &node.dynamics {
                    for value in &dynamics.input_peaks {
                        samples.extend(value.to_le_bytes());
                    }
                    samples.extend(dynamics.gain_reduction_db.to_le_bytes());
                    samples.extend(dynamics.threshold_db.to_le_bytes());
                }
                Binding {
                    node_id: node.node_id.clone(),
                    port_id: node.port_id.clone(),
                    channels: node.peaks.len(),
                    input_channels: node.dynamics.as_ref().map(|d| d.input_peaks.len()),
                }
            })
            .collect();
        (
            Self {
                protocol_version: meters.protocol_version,
                revision: meters.revision,
                bindings: Some(bindings),
                samples: STANDARD.encode(&samples),
            },
            samples,
        )
    }

    fn unpack(self) -> Result<MeterSnapshot, TelemetryDecodeError> {
        let bindings = self.bindings.ok_or(TelemetryDecodeError::MissingBindings)?;
        let bytes = STANDARD.decode(self.samples)?;
        let expected = bindings
            .iter()
            .try_fold(0_usize, |total, binding| {
                let count = binding
                    .input_channels
                    .map_or(Some(0), |n| n.checked_add(2))?;
                total.checked_add(binding.channels)?.checked_add(count)
            })
            .and_then(|count| count.checked_mul(4));
        if expected != Some(bytes.len()) {
            return Err(TelemetryDecodeError::InvalidLength);
        }
        let mut values = bytes
            .chunks_exact(4)
            .map(|v| f32::from_le_bytes([v[0], v[1], v[2], v[3]]));
        let nodes = bindings
            .into_iter()
            .map(|binding| {
                let peaks = values.by_ref().take(binding.channels).collect();
                let dynamics = binding.input_channels.map(|count| DynamicsMeterSnapshot {
                    input_peaks: values.by_ref().take(count).collect(),
                    gain_reduction_db: values.next().unwrap_or_default(),
                    threshold_db: values.next().unwrap_or_default(),
                });
                NodeMeterSnapshot {
                    node_id: binding.node_id,
                    port_id: binding.port_id,
                    peaks,
                    dynamics,
                }
            })
            .collect();
        Ok(MeterSnapshot {
            protocol_version: self.protocol_version,
            revision: self.revision,
            nodes,
        })
    }
}

mod packed {
    use super::{Deserialize, MeterSnapshot, PackedMeters, Serialize};
    pub(super) fn serialize<S: serde::Serializer>(
        value: &MeterSnapshot,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        PackedMeters::pack(value).0.serialize(serializer)
    }
    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<MeterSnapshot, D::Error> {
        PackedMeters::deserialize(deserializer)?
            .unpack()
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests;
