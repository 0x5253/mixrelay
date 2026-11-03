//! First native process failure, published once without RT allocation or waiting.
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use truce_rack_core::{Error, ProcessFault};

const EMPTY: u32 = 0;
const WRITING: u32 = 1;
const READY: u32 = 2;

#[repr(C)]
pub(super) struct FirstFailure {
    state: AtomicU32,
    kind: AtomicU32,
    code: AtomicI64,
}

impl FirstFailure {
    pub(super) const fn new() -> Self {
        Self {
            state: AtomicU32::new(EMPTY),
            kind: AtomicU32::new(0),
            code: AtomicI64::new(0),
        }
    }

    pub(super) fn record(&self, error: &Error) {
        let fault = match error {
            Error::Process { fault, .. } => *fault,
            Error::Format { code, .. } => ProcessFault::NativeStatus(*code),
            _ => return,
        };
        // One writer reserves the immutable diagnostic; other failures keep their counters.
        if self
            .state
            .compare_exchange(EMPTY, WRITING, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let (kind, code) = encode(fault);
        self.kind.store(kind, Ordering::Relaxed);
        self.code.store(code, Ordering::Relaxed);
        self.state.store(READY, Ordering::Release);
    }

    pub(super) fn read(&self) -> Option<ProcessFault> {
        if self.state.load(Ordering::Acquire) != READY {
            return None;
        }
        Some(match self.kind.load(Ordering::Relaxed) {
            1 => ProcessFault::InputEventCapacity,
            2 => ProcessFault::InvalidInputEvent,
            3 => ProcessFault::OutputEventCapacity,
            4 => ProcessFault::InvalidOutputEvent,
            5 => ProcessFault::InvalidAudioMapping,
            6 => ProcessFault::InvalidBlockSize,
            7 => ProcessFault::MissingCallback,
            8 => ProcessFault::NativeStatus(self.code.load(Ordering::Relaxed)),
            _ => return None,
        })
    }
}

fn encode(fault: ProcessFault) -> (u32, i64) {
    match fault {
        ProcessFault::InputEventCapacity => (1, 0),
        ProcessFault::InvalidInputEvent => (2, 0),
        ProcessFault::OutputEventCapacity => (3, 0),
        ProcessFault::InvalidOutputEvent => (4, 0),
        ProcessFault::InvalidAudioMapping => (5, 0),
        ProcessFault::InvalidBlockSize => (6, 0),
        ProcessFault::MissingCallback => (7, 0),
        ProcessFault::NativeStatus(code) => (8, code),
    }
}

#[cfg(test)]
mod tests;
