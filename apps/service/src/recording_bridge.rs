//! Connects engine take ownership to the independent recording worker registry.

use mixrelay_audio_io::recording::{RecorderRequest, RecorderStatus};
use mixrelay_audio_recorder::{RecorderRegistry, RecorderTap};
use mixrelay_engine::recording_ports::{
    OpenRecording, RecordingBackend, RecordingError, RecordingHold,
};

#[derive(Clone)]
pub(crate) struct RegistryRecordingBackend(pub(crate) RecorderRegistry);
struct HeldTap {
    _tap: RecorderTap,
}
impl RecordingHold for HeldTap {}
impl RecordingBackend for RegistryRecordingBackend {
    fn status(&self, id: &str) -> RecorderStatus {
        self.0.status(id)
    }
    fn open_take_taps(&self) -> Vec<OpenRecording> {
        self.0
            .open_take_taps()
            .into_iter()
            .map(|(id, tap, state)| OpenRecording {
                id,
                hold: Box::new(HeldTap { _tap: tap }),
                state,
            })
            .collect()
    }
    fn command(
        &self,
        id: &str,
        request: RecorderRequest,
    ) -> Result<RecorderStatus, RecordingError> {
        self.0.command(id, request).map_err(RecordingError::new)
    }
    fn stop_all(&self) -> Result<(), RecordingError> {
        self.0.stop_all().map_err(RecordingError::new)
    }
    fn reap_unused(&self) -> Result<(), RecordingError> {
        self.0.reap_unused().map_err(RecordingError::new)
    }
}

#[cfg(test)]
mod tests;
