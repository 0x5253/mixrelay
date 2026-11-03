use std::{
    sync::{Mutex, MutexGuard, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};

use crate::{
    PROCESSOR_STATE_TIMEOUT, ProcessorStateError, STATE_CAPACITY, STATE_FAILED, STATE_OK,
    STATE_TOO_LARGE, STATE_UNSUPPORTED, WORKER_READY,
};

use super::{CHUNK_CAPACITY, Mapping, SharedHeader};

/// Control-only mapping; neither state mutex is reachable from an audio endpoint.
pub(crate) struct ControlMailbox {
    mapping: Mapping,
    writer: Mutex<()>,
}

/// Broker-only response owner. It cannot issue parent requests or borrow PCM.
pub(crate) struct WorkerControl {
    mapping: Mapping,
    response_writer: Mutex<()>,
}

pub(crate) struct ControlGuard<'a> {
    mailbox: &'a ControlMailbox,
    _writer: MutexGuard<'a, ()>,
}

impl ControlMailbox {
    pub(super) const fn from_mapping(mapping: Mapping) -> Self {
        Self {
            mapping,
            writer: Mutex::new(()),
        }
    }

    pub(crate) fn header(&self) -> &SharedHeader {
        self.mapping.header()
    }

    pub(crate) fn last_request_kind(&self) -> u32 {
        self.header().state_kind.load(Ordering::Acquire)
    }

    pub(crate) fn has_pending_request(&self) -> bool {
        let header = self.header();
        header.state_request.load(Ordering::Acquire)
            != header.state_response.load(Ordering::Acquire)
    }

    pub(crate) fn lock(&self) -> ControlGuard<'_> {
        ControlGuard {
            mailbox: self,
            _writer: self
                .writer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        }
    }

    pub(crate) fn try_lock(&self) -> Result<ControlGuard<'_>, ProcessorStateError> {
        Ok(ControlGuard {
            mailbox: self,
            _writer: self
                .writer
                .try_lock()
                .map_err(|_| ProcessorStateError::Busy)?,
        })
    }
}

impl WorkerControl {
    pub(super) const fn from_mapping(mapping: Mapping) -> Self {
        Self {
            mapping,
            response_writer: Mutex::new(()),
        }
    }

    pub(crate) fn header(&self) -> &SharedHeader {
        self.mapping.header()
    }

    pub(crate) fn pending_request(&self) -> Option<(u64, u32)> {
        let header = self.header();
        let request = header.state_request.load(Ordering::Acquire);
        (request != header.state_response.load(Ordering::Acquire))
            .then(|| (request, header.state_kind.load(Ordering::Relaxed)))
    }

    /// Whether the parent abandoned this exact request generation.
    pub(crate) fn request_cancelled(&self, request: u64) -> bool {
        self.header().state_cancel.load(Ordering::Acquire) == request
    }

    /// Native capture runs before this call. Reentrant/late editor results must not overwrite
    /// an acknowledged response while the parent copies it or owns the next request.
    pub(crate) fn publish(&self, request: u64, state: Result<Vec<u8>, ProcessorStateError>) {
        let Ok(_writer) = self.response_writer.try_lock() else {
            return;
        };
        let header = self.header();
        if !request_fence(header, request) {
            return;
        }
        let (status, length) = if can_publish(header, request) {
            match state {
                Ok(bytes) => match u32::try_from(bytes.len()) {
                    Ok(length) if bytes.len() <= STATE_CAPACITY => {
                        if self.publish_chunks(header, request, &bytes) {
                            (STATE_OK, length)
                        } else {
                            (STATE_FAILED, 0)
                        }
                    }
                    _ => (STATE_TOO_LARGE, 0),
                },
                Err(ProcessorStateError::Unsupported) => (STATE_UNSUPPORTED, 0),
                Err(_) => (STATE_FAILED, 0),
            }
        } else {
            (STATE_FAILED, 0)
        };
        if !request_fence(header, request) {
            return;
        }
        header.state_length.store(length, Ordering::Relaxed);
        header.state_result.store(status, Ordering::Relaxed);
        header.state_response.store(request, Ordering::Release);
    }

    fn publish_chunks(&self, header: &SharedHeader, request: u64, bytes: &[u8]) -> bool {
        let deadline = Instant::now() + PROCESSOR_STATE_TIMEOUT;
        let mut offset = 0;
        while offset < bytes.len() {
            if !Self::wait_for_chunk(header, request, deadline) {
                return false;
            }
            let end = offset + bytes.len().saturating_sub(offset).min(CHUNK_CAPACITY);
            if !can_publish(header, request) {
                return false;
            }
            // `publish` admitted only a state whose whole length fits u32.
            let Ok(written) = u32::try_from(end) else {
                return false;
            };
            self.mapping.write_state_chunk(&bytes[offset..end]);
            header.state_written.store(written, Ordering::Release);
            offset = end;
        }
        can_publish(header, request)
    }

    fn wait_for_chunk(header: &SharedHeader, request: u64, deadline: Instant) -> bool {
        loop {
            if !can_publish(header, request) {
                return false;
            }
            let written = header.state_written.load(Ordering::Acquire);
            let consumed = header.state_consumed.load(Ordering::Acquire);
            if consumed > written {
                return false;
            }
            if Instant::now() >= deadline {
                return false;
            }
            if written == consumed {
                return true;
            }
            thread::sleep(Duration::from_micros(100));
        }
    }
}

impl ControlGuard<'_> {
    pub(crate) fn request_blob(
        &mut self,
        kind: u32,
        timeout: Duration,
    ) -> Result<Vec<u8>, ProcessorStateError> {
        let header = self.mailbox.header();
        if header.worker_status.load(Ordering::Acquire) != WORKER_READY
            || header.shutdown.load(Ordering::Acquire) != 0
        {
            return Err(ProcessorStateError::Unavailable);
        }
        let previous = header.state_request.load(Ordering::Relaxed);
        if previous != header.state_response.load(Ordering::Acquire) {
            return Err(ProcessorStateError::Pending);
        }
        let request = next_request(previous);
        header.state_written.store(0, Ordering::Relaxed);
        header.state_consumed.store(0, Ordering::Relaxed);
        header.state_cancel.store(0, Ordering::Relaxed);
        header.state_kind.store(kind, Ordering::Relaxed);
        header.state_request.store(request, Ordering::Release);
        let deadline = Instant::now() + timeout.min(PROCESSOR_STATE_TIMEOUT);
        let mut bytes = Vec::new();
        let mut consumed = 0_u32;
        loop {
            let terminal = header.state_response.load(Ordering::Acquire) == request;
            if Instant::now() >= deadline {
                header.state_cancel.store(request, Ordering::Release);
                return Err(ProcessorStateError::Timeout);
            }
            self.drain_chunk(header, request, &mut bytes, &mut consumed)?;
            if terminal {
                break;
            }
            if header.worker_status.load(Ordering::Acquire) != WORKER_READY
                || header.shutdown.load(Ordering::Acquire) != 0
            {
                header.state_cancel.store(request, Ordering::Release);
                return Err(ProcessorStateError::Unavailable);
            }
            thread::sleep(Duration::from_micros(100));
        }
        if Instant::now() >= deadline {
            header.state_cancel.store(request, Ordering::Release);
            return Err(ProcessorStateError::Timeout);
        }
        let length = header.state_length.load(Ordering::Relaxed) as usize;
        match header.state_result.load(Ordering::Relaxed) {
            STATE_OK => {
                if length > STATE_CAPACITY || length != bytes.len() {
                    return Err(ProcessorStateError::InvalidResponse);
                }
                Ok(bytes)
            }
            STATE_TOO_LARGE => Err(ProcessorStateError::TooLarge),
            STATE_UNSUPPORTED => Err(ProcessorStateError::Unsupported),
            _ => Err(ProcessorStateError::NativeFailure),
        }
    }

    fn drain_chunk(
        &self,
        header: &SharedHeader,
        request: u64,
        bytes: &mut Vec<u8>,
        consumed: &mut u32,
    ) -> Result<(), ProcessorStateError> {
        let written = header.state_written.load(Ordering::Acquire);
        if written < *consumed {
            header.state_cancel.store(request, Ordering::Release);
            return Err(ProcessorStateError::InvalidResponse);
        }
        let delta = written - *consumed;
        if delta as usize > CHUNK_CAPACITY {
            header.state_cancel.store(request, Ordering::Release);
            return Err(ProcessorStateError::InvalidResponse);
        }
        let total = written as usize;
        if total > STATE_CAPACITY {
            header.state_cancel.store(request, Ordering::Release);
            return Err(ProcessorStateError::InvalidResponse);
        }
        if delta != 0 {
            let delta = delta as usize;
            if bytes.try_reserve(delta).is_err() {
                header.state_cancel.store(request, Ordering::Release);
                return Err(ProcessorStateError::NativeFailure);
            }
            self.mailbox.mapping.append_state_chunk(bytes, delta);
            *consumed = written;
            header.state_consumed.store(written, Ordering::Release);
        }
        Ok(())
    }
}

fn request_fence(header: &SharedHeader, request: u64) -> bool {
    header.state_request.load(Ordering::Acquire) == request
        && header.state_response.load(Ordering::Acquire) != request
}

fn can_publish(header: &SharedHeader, request: u64) -> bool {
    request_fence(header, request)
        && header.state_cancel.load(Ordering::Acquire) != request
        && header.shutdown.load(Ordering::Acquire) == 0
}

fn next_request(previous: u64) -> u64 {
    let next = previous.wrapping_add(1);
    if next == 0 { 1 } else { next }
}
