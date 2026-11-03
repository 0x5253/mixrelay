use serde::Serialize;
use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::Path,
};

pub(super) const PHASE_COUNT: usize = 3;
pub(super) const PHASE_FRAMES: usize = 24_000;
pub(super) const GRAPH_CONTINUITY_FRAMES: usize = 480_000;
const CHANNELS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TraceConfig {
    pub(super) phase_count: usize,
    pub(super) phase_frames: usize,
    file_prefix: &'static str,
}

impl TraceConfig {
    pub(super) const fn source_mute() -> Self {
        Self {
            phase_count: PHASE_COUNT,
            phase_frames: PHASE_FRAMES,
            file_prefix: "mute-sink",
        }
    }

    pub(super) const fn graph_continuity() -> Self {
        Self {
            phase_count: 1,
            phase_frames: GRAPH_CONTINUITY_FRAMES,
            file_prefix: "graph-sink",
        }
    }

    fn sample_count(self) -> Result<usize, SinkError> {
        let frames = self
            .phase_count
            .checked_mul(self.phase_frames)
            .filter(|frames| *frames <= GRAPH_CONTINUITY_FRAMES)
            .ok_or_else(|| SinkError::Fixture("observer PCM capacity is invalid".into()))?;
        frames
            .checked_mul(CHANNELS)
            .ok_or_else(|| SinkError::Fixture("observer PCM capacity overflow".into()))
    }
}

#[derive(Serialize)]
struct SinkMetadata<'a> {
    sample_rate: u32,
    channels: u32,
    phase_frames: usize,
    frame_counts: &'a [u32],
    empty_frames: &'a [u32],
    invalid_buffers: u64,
    post_window_frames: &'a [u64],
    raw_file: String,
    process_id: u32,
    mode: &'static str,
}

/// Fixed raw PCM storage, allocated before native callbacks are registered.
pub(super) struct Trace {
    pub(super) samples: Box<[f32]>,
    pub(super) frame_counts: Box<[u32]>,
    pub(super) empty_frames: Box<[u32]>,
    pub(super) invalid_buffers: u64,
    pub(super) post_window_frames: Box<[u64]>,
    pub(super) config: TraceConfig,
}

impl Trace {
    pub(super) fn new(config: TraceConfig) -> Result<Self, SinkError> {
        let samples = config.sample_count()?;
        Ok(Self {
            samples: vec![0.0; samples].into_boxed_slice(),
            frame_counts: vec![0; config.phase_count].into_boxed_slice(),
            empty_frames: vec![0; config.phase_count].into_boxed_slice(),
            invalid_buffers: 0,
            post_window_frames: vec![0; config.phase_count].into_boxed_slice(),
            config,
        })
    }

    /// Writes the fixed-capacity raw trace only after disconnect drained callbacks.
    /// # Errors
    /// Rejects pre-existing evidence paths and propagates encoding or file errors.
    pub(super) fn save(&self, executable: &Path, process_id: u32) -> Result<(), SinkError> {
        let raw_path =
            executable.with_file_name(format!("{}-{process_id}.f32", self.config.file_prefix));
        let json_path =
            executable.with_file_name(format!("{}-{process_id}.json", self.config.file_prefix));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(raw_path)?;
        let mut writer = BufWriter::new(file);
        for sample in &self.samples {
            writer.write_all(&sample.to_ne_bytes())?;
        }
        writer.flush()?;

        let raw_file = format!("{}-{process_id}.f32", self.config.file_prefix);
        let mut metadata = BufWriter::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(json_path)?,
        );
        serde_json::to_writer(
            &mut metadata,
            &SinkMetadata {
                sample_rate: 48_000,
                channels: 2,
                phase_frames: self.config.phase_frames,
                frame_counts: &self.frame_counts,
                empty_frames: &self.empty_frames,
                invalid_buffers: self.invalid_buffers,
                post_window_frames: &self.post_window_frames,
                raw_file,
                process_id,
                mode: self.config.file_prefix,
            },
        )?;
        metadata.flush()?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum SinkError {
    #[error("native mute sink: {0}")]
    Native(#[from] pipewire::Error),
    #[error("mute sink evidence: {0}")]
    Io(#[from] std::io::Error),
    #[error("mute sink evidence JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("mute sink fixture: {0}")]
    Fixture(String),
    #[error("mute sink trace was not returned after disconnect")]
    TraceReturn,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_capacity_is_bounded_for_legacy_and_full_graph_windows() {
        assert_eq!(TraceConfig::source_mute().sample_count().unwrap(), 144_000);
        assert_eq!(
            TraceConfig::graph_continuity().sample_count().unwrap(),
            GRAPH_CONTINUITY_FRAMES * CHANNELS
        );
    }

    #[test]
    fn trace_capacity_rejects_overflow_and_windows_above_ten_seconds() {
        for config in [
            TraceConfig {
                phase_count: usize::MAX,
                phase_frames: 2,
                file_prefix: "invalid",
            },
            TraceConfig {
                phase_count: 2,
                phase_frames: GRAPH_CONTINUITY_FRAMES / 2 + 1,
                file_prefix: "invalid",
            },
        ] {
            assert!(config.sample_count().is_err());
        }
    }
}
