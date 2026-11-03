//! Native device formats, distinct from graph DSP and recorder sample formats.

use serde::{Deserialize, Serialize};

/// The device's native PCM representation, not an OS-converted client format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeSampleFormat {
    /// IEEE 754 single-precision PCM.
    Float32,
    /// Signed 16-bit integer PCM.
    Signed16,
    /// Packed signed 24-bit integer PCM.
    Signed24,
    /// Signed 32-bit integer PCM.
    Signed32,
}

/// One supported combination; rate, channel count and representation stay paired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeStreamFormat {
    /// Native samples per second.
    pub sample_rate: u32,
    /// Native channels in this direction.
    pub channels: u16,
    /// PCM representation advertised by the native endpoint.
    pub sample_format: NativeSampleFormat,
}

/// Observed and supported formats for one native device direction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeStreamFormats {
    /// Actual readback, or `None` when the native format cannot be observed.
    pub current: Option<NativeStreamFormat>,
    /// Exact native combinations. Empty means inactive or not discovered.
    pub supported: Vec<NativeStreamFormat>,
}

/// Separate OS-playback and OS-capture native format capabilities.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeDeviceFormats {
    /// OS playback entering the virtual device.
    pub render: NativeStreamFormats,
    /// Virtual-device audio captured by OS applications.
    pub capture: NativeStreamFormats,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_native_formats_do_not_invent_defaults() {
        let formats = NativeDeviceFormats::default();
        assert!(formats.render.current.is_none());
        assert!(formats.capture.current.is_none());
        assert!(formats.render.supported.is_empty());
        assert!(formats.capture.supported.is_empty());
    }

    #[test]
    fn serialized_formats_keep_directional_combinations_and_pcm_types() {
        let float = NativeStreamFormat {
            sample_rate: 48_000,
            channels: 32,
            sample_format: NativeSampleFormat::Float32,
        };
        let integer = NativeStreamFormat {
            sample_rate: 44_100,
            channels: 2,
            sample_format: NativeSampleFormat::Signed32,
        };
        let formats = NativeDeviceFormats {
            render: NativeStreamFormats {
                current: Some(float),
                supported: vec![float, integer],
            },
            capture: NativeStreamFormats {
                current: None,
                supported: vec![integer],
            },
        };
        let json = serde_json::to_value(&formats).unwrap();
        assert_eq!(json["render"]["current"]["sampleFormat"], "float32");
        assert_eq!(json["capture"]["supported"][0]["sampleFormat"], "signed32");
        assert_eq!(json["render"]["supported"][1]["sampleRate"], 44_100);
        assert_eq!(json["render"]["supported"][1]["channels"], 2);
        assert_eq!(
            serde_json::from_value::<NativeDeviceFormats>(json).unwrap(),
            formats
        );
    }

    #[test]
    fn sixteen_and_twenty_four_bit_pcm_have_distinct_contract_values() {
        for (format, label) in [
            (NativeSampleFormat::Signed16, "signed16"),
            (NativeSampleFormat::Signed24, "signed24"),
        ] {
            assert_eq!(serde_json::to_value(format).unwrap(), label);
            assert_eq!(
                serde_json::from_value::<NativeSampleFormat>(serde_json::json!(label)).unwrap(),
                format
            );
        }
    }
}
