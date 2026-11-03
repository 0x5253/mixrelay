use super::*;
use crate::recording_ports::OpenRecording;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};

#[derive(Default)]
struct Recordings {
    states: Mutex<BTreeMap<String, RecorderStatus>>,
    held: Arc<AtomicUsize>,
    fail_resume: Mutex<bool>,
    commands: Mutex<Vec<(String, RecorderAction)>>,
}
struct Hold(Arc<AtomicUsize>);
impl RecordingHold for Hold {}
impl Drop for Hold {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl RecordingBackend for Recordings {
    fn status(&self, id: &str) -> RecorderStatus {
        self.states.lock().get(id).cloned().unwrap_or_default()
    }
    fn open_take_taps(&self) -> Vec<OpenRecording> {
        self.states
            .lock()
            .iter()
            .filter(|(_, status)| {
                matches!(
                    status.state,
                    RecorderState::Recording | RecorderState::Paused
                )
            })
            .map(|(id, status)| {
                self.held.fetch_add(1, Ordering::Relaxed);
                OpenRecording {
                    id: id.clone(),
                    hold: Box::new(Hold(self.held.clone())),
                    state: status.state,
                }
            })
            .collect()
    }
    fn command(
        &self,
        id: &str,
        request: RecorderRequest,
    ) -> Result<RecorderStatus, RecordingError> {
        self.commands.lock().push((id.to_owned(), request.action));
        if request.action == RecorderAction::Record && *self.fail_resume.lock() {
            return Err(RecordingError::new(std::io::Error::other(
                "resume rejected",
            )));
        }
        let mut recordings = self.states.lock();
        let status = recordings.get_mut(id).unwrap();
        status.state = match request.action {
            RecorderAction::Record => RecorderState::Recording,
            RecorderAction::Pause => RecorderState::Paused,
            RecorderAction::Stop => RecorderState::Stopped,
        };
        Ok(status.clone())
    }
    fn stop_all(&self) -> Result<(), RecordingError> {
        self.states
            .lock()
            .values_mut()
            .for_each(|status| status.state = RecorderState::Stopped);
        Ok(())
    }
    fn reap_unused(&self) -> Result<(), RecordingError> {
        Ok(())
    }
}

fn recordings() -> Arc<Recordings> {
    let recordings = Arc::new(Recordings::default());
    for (id, state) in [
        ("live", RecorderState::Recording),
        ("paused", RecorderState::Paused),
    ] {
        recordings.states.lock().insert(
            id.to_owned(),
            RecorderStatus {
                state,
                ..RecorderStatus::default()
            },
        );
    }
    recordings
}

#[test]
fn an_empty_paused_graph_retains_open_takes_and_resumes_only_previously_recording_takes() {
    let recordings = recordings();
    let sessions = RecorderSessions::new(recordings.clone());
    sessions.suspend().unwrap();
    assert_eq!(recordings.held.load(Ordering::Relaxed), 2);
    assert_eq!(recordings.status("live").state, RecorderState::Paused);
    sessions.resume().unwrap();
    assert_eq!(recordings.status("live").state, RecorderState::Recording);
    assert_eq!(recordings.status("paused").state, RecorderState::Paused);
    assert_eq!(recordings.held.load(Ordering::Relaxed), 0);
    assert_eq!(
        *recordings.commands.lock(),
        [
            ("live".into(), RecorderAction::Pause),
            ("live".into(), RecorderAction::Record)
        ]
    );
}

#[test]
fn a_rejected_resume_retains_its_take_for_the_next_attempt() {
    let recordings = recordings();
    let sessions = RecorderSessions::new(recordings.clone());
    sessions.suspend().unwrap();
    *recordings.fail_resume.lock() = true;
    assert!(sessions.resume().is_err());
    assert_eq!(recordings.status("live").state, RecorderState::Paused);
    assert_eq!(recordings.held.load(Ordering::Relaxed), 1);
    *recordings.fail_resume.lock() = false;
    sessions.resume().unwrap();
    assert_eq!(recordings.status("live").state, RecorderState::Recording);
    assert_eq!(recordings.held.load(Ordering::Relaxed), 0);
}
