//! Session identity, channel admission, capacity and retirement over owned worker handles.

use crate::{RecorderError, RecorderFailure};
use crate::{RecorderRequest, RecorderState, RecorderStatus, RecorderTap};
use parking_lot::Mutex;
use std::collections::BTreeMap;

const MAX_SESSIONS: usize = 16;

/// Prepares an owned worker handle and its first real transport tap.
pub(crate) trait SessionFactory {
    type Handle: SessionHandle;
    fn prepare(&self, channels: u16) -> Result<(Self::Handle, RecorderTap), RecorderError>;
}

/// Control operations consumed by session admission and retirement.
pub(crate) trait SessionHandle {
    fn channels(&self) -> u16;
    fn live_taps(&self) -> usize;
    fn tap(&self) -> RecorderTap;
    fn status(&self) -> RecorderStatus;
    fn command(&self, request: RecorderRequest) -> Result<RecorderStatus, RecorderError>;
    /// Finalizes and joins once; subsequent calls succeed without repeating cleanup.
    fn shutdown(&mut self) -> Result<(), RecorderError>;
}

pub(crate) struct Sessions<F: SessionFactory> {
    factory: F,
    handles: Mutex<BTreeMap<String, F::Handle>>,
    reap_error: Mutex<Option<RecorderError>>,
}

impl<F: SessionFactory> Sessions<F> {
    pub(crate) fn new(factory: F) -> Self {
        Self {
            factory,
            handles: Mutex::new(BTreeMap::new()),
            reap_error: Mutex::new(None),
        }
    }

    /// Prepare a stable tap for a graph-local runtime identity, off the audio thread.
    /// Discarding a failed graph preparation never stops an existing active tap.
    /// # Errors
    /// Rejects empty IDs, channel counts outside the shared recording limit, capacity exhaustion,
    /// channel changes while a previous tap/take exists, or worker spawn failure.
    pub(crate) fn prepare(&self, id: &str, channels: u16) -> Result<RecorderTap, RecorderError> {
        if id.trim().is_empty()
            || !(1..=mixrelay_audio_io::formats::MAX_RECORDING_CHANNELS).contains(&channels)
        {
            return Err(RecorderFailure::InvalidIdentity.into());
        }
        let mut sessions = self.handles.lock();
        if let Some(session) = sessions.get(id) {
            if session.channels() == channels {
                return Ok(session.tap());
            }
            if session.live_taps() != 0
                || matches!(
                    session.status().state,
                    RecorderState::Recording | RecorderState::Paused
                )
            {
                return Err(RecorderFailure::ActiveChannels.into());
            }
        }
        if let Some(mut replaced) = sessions.remove(id)
            && let Err(error) = replaced.shutdown()
        {
            store_reap_error(&self.reap_error, describe_error(&replaced, id, error));
        }
        if sessions.len() >= MAX_SESSIONS {
            reap_sessions(&mut sessions, &self.reap_error);
        }
        if sessions.len() >= MAX_SESSIONS {
            return Err(RecorderFailure::Capacity.into());
        }
        let (session, tap) = self.factory.prepare(channels)?;
        sessions.insert(id.to_owned(), session);
        Ok(tap)
    }

    /// Returns the latest writer state; an unknown identity is stopped/unconfigured.
    #[must_use]
    pub(crate) fn status(&self, id: &str) -> RecorderStatus {
        self.handles
            .lock()
            .get(id)
            .map_or_else(RecorderStatus::default, SessionHandle::status)
    }

    /// Retains open takes while an owner temporarily retires its render plans.
    /// These taps prevent worker retirement; they do not admit PCM by themselves.
    #[must_use]
    pub(crate) fn open_take_taps(&self) -> Vec<(String, RecorderTap, RecorderState)> {
        self.handles
            .lock()
            .iter()
            .filter_map(|(id, session)| {
                let state = session.status().state;
                matches!(state, RecorderState::Recording | RecorderState::Paused)
                    .then(|| (id.clone(), session.tap(), state))
            })
            .collect()
    }

    /// Execute and acknowledge a writer command on the control thread.
    /// Stop/Pause cut off producers, drain accepted packets and flush conversion
    /// tails before returning. Stop also finalizes and closes the file.
    /// # Errors
    /// Reports absent identities, invalid transitions, file/conversion failures,
    /// and disconnected writer threads. These errors never stop the audio graph.
    pub(crate) fn command(
        &self,
        id: &str,
        request: RecorderRequest,
    ) -> Result<RecorderStatus, RecorderError> {
        self.handles
            .lock()
            .get(id)
            .ok_or(RecorderFailure::Unprepared)?
            .command(request)
    }

    /// Stop/drain/join sessions with no live taps after render-plan retirement.
    /// No caller-supplied node list can invalidate an active or pending plan.
    /// # Errors
    /// Reports worker shutdown failures after all eligible sessions are reaped.
    pub(crate) fn reap_unused(&self) -> Result<(), RecorderError> {
        reap_sessions(&mut self.handles.lock(), &self.reap_error);
        self.reap_error.lock().take().map_or(Ok(()), Err)
    }

    /// Finalize every take during graceful service shutdown, even while render
    /// plans still own taps. Workers remain registered and all taps stay stopped.
    /// # Errors
    /// Aggregates recorder identities, paths and any drain/finalization failures.
    pub(crate) fn stop_all(&self) -> Result<(), RecorderError> {
        let mut failures = Vec::new();
        for (id, session) in self.handles.lock().iter() {
            let result = session
                .command(RecorderRequest {
                    action: crate::RecorderAction::Stop,
                    path: None,
                    sample_rate: None,
                    encoding: None,
                })
                .map(|_| ());
            if let Err(error) = result {
                failures.push(describe_error(session, id, error));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(RecorderFailure::Sessions(failures).into())
        }
    }
}

impl<F: SessionFactory> Drop for Sessions<F> {
    fn drop(&mut self) {
        for session in self.handles.get_mut().values_mut() {
            let _ = session.shutdown();
        }
    }
}

fn reap_sessions<H: SessionHandle>(
    sessions: &mut BTreeMap<String, H>,
    error: &Mutex<Option<RecorderError>>,
) {
    sessions.retain(|id, session| {
        if session.live_taps() != 0 {
            return true;
        }
        if let Err(failure) = session.shutdown() {
            store_reap_error(error, describe_error(session, id, failure));
        }
        false
    });
}

fn store_reap_error(slot: &Mutex<Option<RecorderError>>, failure: RecorderError) {
    let mut error = slot.lock();
    *error = Some(match error.take() {
        Some(previous) => previous.combine(failure),
        None => failure,
    });
}

fn describe_error(session: &impl SessionHandle, id: &str, source: RecorderError) -> RecorderError {
    RecorderFailure::Session {
        id: id.to_owned(),
        path: session.status().path,
        source,
    }
    .into()
}

#[cfg(test)]
mod tests;
