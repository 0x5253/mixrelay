//! Recording pause, resume and retirement policy over a retained take boundary.

use crate::recording_ports::{RecordingBackend, RecordingError, RecordingHold};
use mixrelay_audio_graph::NodeId;
use mixrelay_audio_io::recording::{
    RecorderAction, RecorderRequest, RecorderState, RecorderStatus,
};
use mixrelay_control_protocol::{ErrorCode, ProtocolError};
use parking_lot::Mutex;
use std::sync::Arc;

pub(crate) fn recorder_error(reason: impl std::fmt::Display) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::RecorderFailed,
        parameters: serde_json::json!({"reason": serde_json::Value::String(reason.to_string())}),
    }
}

pub(crate) struct RecorderSessions {
    registry: Arc<dyn RecordingBackend>,
    suspended: Mutex<Vec<SuspendedRecorder>>,
    cleanup_error: Mutex<Option<RecordingError>>,
}

struct SuspendedRecorder {
    id: String,
    // The empty paused graph owns no taps. Keep the open take alive until resume.
    _tap: Box<dyn RecordingHold>,
    resume: bool,
}

fn recorder_request(action: RecorderAction) -> RecorderRequest {
    RecorderRequest {
        action,
        path: None,
        sample_rate: None,
        encoding: None,
    }
}

impl RecorderSessions {
    pub(crate) fn new(registry: Arc<dyn RecordingBackend>) -> Self {
        Self {
            registry,
            suspended: Mutex::new(Vec::new()),
            cleanup_error: Mutex::new(None),
        }
    }

    pub(crate) fn status(&self, id: &str) -> RecorderStatus {
        self.registry.status(id)
    }

    pub(crate) fn control(
        &self,
        id: &str,
        request: RecorderRequest,
    ) -> Result<RecorderStatus, ProtocolError> {
        self.registry.command(id, request).map_err(recorder_error)
    }

    pub(crate) fn stop_inactive(&self, id: &str, graph_id: &str, node_id: &NodeId) {
        if let Err(reason) = self
            .registry
            .command(id, recorder_request(RecorderAction::Stop))
        {
            tracing::error!(%graph_id, %node_id, %reason, "disabled recorder finalization failed");
            *self.cleanup_error.lock() = Some(reason);
        }
    }

    pub(crate) fn finalize_all(&self) -> Result<(), ProtocolError> {
        self.registry.stop_all().map_err(recorder_error)
    }

    pub(crate) fn cleanup_error(&self) -> Option<String> {
        self.cleanup_error.lock().as_ref().map(ToString::to_string)
    }

    pub(crate) fn reclaim(&self) {
        self.suspended.lock().retain(|recorder| {
            matches!(
                self.registry.status(&recorder.id).state,
                RecorderState::Recording | RecorderState::Paused
            )
        });
        if let Err(reason) = self.registry.reap_unused() {
            tracing::error!(%reason, "recorder finalization failed");
            *self.cleanup_error.lock() = Some(reason);
        }
    }

    pub(crate) fn suspend(&self) -> Result<(), ProtocolError> {
        let mut held = self.suspended.lock();
        *held = self
            .registry
            .open_take_taps()
            .into_iter()
            .map(|take| SuspendedRecorder {
                id: take.id,
                _tap: take.hold,
                resume: take.state == RecorderState::Recording,
            })
            .collect();
        for recorder in held.iter().filter(|recorder| recorder.resume) {
            if let Err(reason) = self
                .registry
                .command(&recorder.id, recorder_request(RecorderAction::Pause))
            {
                drop(held);
                let rollback = self.resume();
                return Err(recorder_error(format!(
                    "cannot pause recording: {reason}; rollback: {rollback:?}"
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn resume(&self) -> Result<(), ProtocolError> {
        let mut held = self.suspended.lock();
        let mut failures = Vec::new();
        held.retain(|recorder| {
            if !recorder.resume || self.registry.status(&recorder.id).state != RecorderState::Paused
            {
                return false;
            }
            match self
                .registry
                .command(&recorder.id, recorder_request(RecorderAction::Record))
            {
                Ok(_) => false,
                Err(reason) => {
                    failures.push(format!("{}: {reason}", recorder.id));
                    true
                }
            }
        });
        if failures.is_empty() {
            Ok(())
        } else {
            Err(recorder_error(failures.join("; ")))
        }
    }
}

#[cfg(test)]
mod tests;
