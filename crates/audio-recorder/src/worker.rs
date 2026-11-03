//! Recording transitions and accepted PCM drainage over injected destinations.

use crate::{
    RecorderAction, RecorderRequest, RecorderState, RecorderStatus,
    recording_files::FileFactory,
    transport::{self, MAX_TOKEN, PACKETS, Packet, Transport},
    wave::Take,
};
use crate::{RecorderError, RecorderFailure};
use parking_lot::Mutex;
use std::sync::{Arc, atomic::Ordering};

#[derive(Clone, Copy, PartialEq, Eq)]
enum StopMode {
    Pause,
    Finish,
}

pub(crate) struct Worker<F: FileFactory> {
    transport: Arc<Transport>,
    status: Arc<Mutex<RecorderStatus>>,
    take: Option<Take<F>>,
    files: F,
    failure: Option<RecorderError>,
    token: u64,
    packet: Packet,
}

impl<F: FileFactory> Worker<F> {
    pub(crate) fn new(
        transport: Arc<Transport>,
        status: Arc<Mutex<RecorderStatus>>,
        files: F,
    ) -> Result<Self, std::collections::TryReserveError> {
        Ok(Self {
            packet: Packet::new(usize::from(transport.channels))?,
            transport,
            status,
            take: None,
            files,
            failure: None,
            token: 0,
        })
    }

    pub(crate) fn service(&mut self) {
        if let Some(error) = transport::fault(self.transport.gate.load(Ordering::SeqCst)) {
            self.fail_admission(error.into());
        }
        if let Err(error) = self.drain() {
            self.fail(error);
        }
        // Retirement is still serviced when no UI/status client is polling.
        if self.transport.taps.load(Ordering::SeqCst) == 0 && self.take.is_some() {
            let _ = self.stop(StopMode::Finish);
        }
    }

    pub(crate) fn has_take(&self) -> bool {
        self.take.is_some()
    }

    pub(crate) fn command(
        &mut self,
        request: &RecorderRequest,
    ) -> Result<RecorderStatus, RecorderError> {
        let result = match request.action {
            RecorderAction::Record => self.record(request),
            RecorderAction::Pause => self.stop(StopMode::Pause),
            RecorderAction::Stop => self.stop(StopMode::Finish),
        };
        result?;
        let status = self.status.lock().clone();
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        Ok(status)
    }

    fn record(&mut self, request: &RecorderRequest) -> Result<(), RecorderError> {
        let status = self.status.lock().clone();
        if status.state == RecorderState::Recording {
            return Err(RecorderFailure::AlreadyRecording.into());
        }
        if self.transport.taps.load(Ordering::SeqCst) == 0 {
            return Err(RecorderFailure::NoTap.into());
        }
        let next = self
            .token
            .checked_add(1)
            .filter(|token| *token <= MAX_TOKEN)
            .ok_or(RecorderFailure::AdmissionExhausted)?;
        if status.state == RecorderState::Paused {
            validate_resume(request, &status)?;
            let observed = self.transport.rate.load(Ordering::Acquire);
            if let Some(take) = &mut self.take
                && let Err(error) = take.prepare_rate(observed)
            {
                return Err(self.fail(error));
            }
        } else {
            let observed = self.transport.rate.load(Ordering::Acquire);
            let channels = self.transport.channels;
            *self.status.lock() = RecorderStatus {
                path: request.path.clone(),
                sample_rate: request
                    .sample_rate
                    .or(Some(observed).filter(|rate| *rate != 0)),
                encoding: Some(request.encoding.unwrap_or(crate::Encoding::Pcm24)),
                ..RecorderStatus::default()
            };
            let take = match Take::new(request, channels, observed, &mut self.files) {
                Ok(take) => take,
                Err(error) => {
                    return Err(self.fail(error));
                }
            };
            *self.status.lock() = RecorderStatus {
                state: RecorderState::Stopped,
                path: request.path.clone(),
                sample_rate: Some(take.rate()),
                encoding: Some(take.encoding()),
                frames: 0,
                error: None,
            };
            self.take = Some(take);
            self.failure = None;
        }
        self.token = next;
        self.status.lock().state = RecorderState::Recording;
        self.transport.gate.store(next, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&mut self, mode: StopMode) -> Result<(), RecorderError> {
        if let Some(error) = self.transport.cutoff() {
            return Err(self.fail_admission(error.into()));
        }
        if let Err(error) = self.drain() {
            return Err(self.fail(error));
        }
        let result = if mode == StopMode::Pause {
            self.take
                .as_mut()
                .map_or(Ok(()), Take::flush_segment)
                .map(|()| {
                    self.take
                        .as_ref()
                        .map_or(self.status.lock().frames, Take::frames)
                })
        } else {
            self.take
                .take()
                .map_or(Ok(self.status.lock().frames), Take::finish)
        };
        match result {
            Ok(frames) => {
                let mut status = self.status.lock();
                status.frames = frames;
                if status.state != RecorderState::Failed {
                    status.state = if mode == StopMode::Pause && self.take.is_some() {
                        RecorderState::Paused
                    } else {
                        RecorderState::Stopped
                    };
                }
                Ok(())
            }
            Err(error) => Err(self.fail(error)),
        }
    }

    /// Writes accepted packets, preserving their prefix when an admission fault occurs.
    ///
    /// # Errors
    /// Returns the writer failure; that ends the recording at once.
    fn drain(&mut self) -> Result<(), RecorderError> {
        for _ in 0..PACKETS {
            if !self.transport.read_packet(&mut self.packet) {
                break;
            }
            let packet = &self.packet;
            if let Some(take) = &mut self.take {
                take.write(
                    &packet.samples[..packet.frames * usize::from(self.transport.channels)],
                    packet.rate,
                )?;
            }
        }
        if let Some(take) = &self.take {
            self.status.lock().frames = take.frames();
        }
        Ok(())
    }

    fn fail(&mut self, mut error: RecorderError) -> RecorderError {
        self.transport.cutoff();
        for _ in 0..PACKETS {
            if !self.transport.read_packet(&mut self.packet) {
                break;
            }
        }
        if let Some(take) = self.take.take() {
            self.status.lock().frames = take.frames();
            match take.finish() {
                Ok(frames) => self.status.lock().frames = frames,
                Err(finalize) => {
                    error = error.combine(finalize);
                }
            }
        }
        let mut status = self.status.lock();
        status.state = RecorderState::Failed;
        status.error = Some(error.to_string());
        self.failure = Some(error.clone());
        self.transport.acknowledge_failure();
        error
    }

    fn fail_admission(&mut self, error: RecorderError) -> RecorderError {
        // Admission faults do not invalidate packets already accepted into the
        // queue. Preserve that prefix; a real writer failure ends it immediately.
        self.transport.cutoff();
        let error = match self.drain() {
            Ok(()) => error,
            Err(write) => error.combine(write),
        };
        self.fail(error)
    }

    pub(crate) fn shutdown(&mut self) -> Result<(), RecorderError> {
        let _ = self.stop(StopMode::Finish);
        self.failure.clone().map_or(Ok(()), Err)
    }
}

fn validate_resume(
    request: &RecorderRequest,
    status: &RecorderStatus,
) -> Result<(), RecorderError> {
    if request
        .path
        .as_ref()
        .is_some_and(|path| Some(path) != status.path.as_ref())
        || request
            .sample_rate
            .is_some_and(|rate| Some(rate) != status.sample_rate)
        || request
            .encoding
            .is_some_and(|encoding| Some(encoding) != status.encoding)
    {
        return Err(RecorderFailure::ResumeChanged.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
