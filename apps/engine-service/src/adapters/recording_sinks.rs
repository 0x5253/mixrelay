//! Connects the engine's recording ports to the file recorder sessions.
use mixrelay_audio_recorder::{RecorderRegistry, RecorderTap};
use mixrelay_render::{RecordingSink, RecordingSinkError, RecordingSinks};

/// The recorder registry as the engine's recording sinks.
pub(super) struct FileRecordingSinks<'a>(pub(super) &'a RecorderRegistry);

struct TapSink(RecorderTap);

impl RecordingSink for TapSink {
    fn capture(&self, samples: &[f32], sample_rate: u32) {
        self.0.capture(samples, sample_rate);
    }
}

impl RecordingSinks for FileRecordingSinks<'_> {
    fn prepare(
        &self,
        session: &str,
        channels: u16,
    ) -> Result<Box<dyn RecordingSink>, RecordingSinkError> {
        match self.0.prepare(session, channels) {
            Ok(tap) => Ok(Box::new(TapSink(tap))),
            Err(reason) => Err(RecordingSinkError(reason.to_string())),
        }
    }
}

#[cfg(test)]
mod tests;
