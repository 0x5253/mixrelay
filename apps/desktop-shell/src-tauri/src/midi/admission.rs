//! Mapping generations and writes share one asynchronous admission gate.
use super::{State, lock};
use std::{
    future::Future,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub(crate) struct Admission<'a> {
    state: &'a Mutex<State>,
    stop: &'a AtomicBool,
    gate: &'a tokio::sync::Mutex<()>,
    generation: u64,
}

impl<'a> Admission<'a> {
    pub(super) fn new(
        state: &'a Mutex<State>,
        stop: &'a AtomicBool,
        gate: &'a tokio::sync::Mutex<()>,
        generation: u64,
    ) -> Self {
        Self {
            state,
            stop,
            gate,
            generation,
        }
    }

    pub(crate) fn current(&self) -> bool {
        let state = lock(self.state);
        !self.stop.load(Ordering::Acquire)
            && !state.map_mode
            && state.store.generation() == self.generation
    }

    pub(crate) async fn enter(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        let guard = self.while_current(self.gate.lock()).await?;
        self.current().then_some(guard)
    }

    pub(crate) async fn while_current<T>(&self, future: impl Future<Output = T>) -> Option<T> {
        tokio::pin!(future);
        loop {
            if !self.current() {
                return None;
            }
            tokio::select! {
                value = &mut future => return Some(value),
                // Match the MIDI worker cadence so blocked reads/waits yield to cancellation.
                () = tokio::time::sleep(Duration::from_millis(5)) => {}
            }
        }
    }
}
