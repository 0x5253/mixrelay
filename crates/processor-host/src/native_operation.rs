//! Per-thread native-call progress shared with the parent supervisor.

use mixrelay_time::Instant;
use std::{
    marker::PhantomData,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

const ACTIVE: u64 = 1;
const STAGE_MASK: u64 = 0xf;
const GENERATION_SHIFT: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Load = 1,
    Process,
    Parameter,
    Service,
    State,
    Editor,
    Destroy,
    EditorEvents,
}

impl Stage {
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Load => "load",
            Self::Process => "process",
            Self::Parameter => "parameter",
            Self::Service => "service",
            Self::State => "state",
            Self::Editor => "editor",
            Self::Destroy => "destroy",
            Self::EditorEvents => "editor_events",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Snapshot(u64);

impl Snapshot {
    pub(super) const fn stage(self) -> Option<Stage> {
        match (self.0 >> 1) & STAGE_MASK {
            1 => Some(Stage::Load),
            2 => Some(Stage::Process),
            3 => Some(Stage::Parameter),
            4 => Some(Stage::Service),
            5 => Some(Stage::State),
            6 => Some(Stage::Editor),
            7 => Some(Stage::Destroy),
            8 => Some(Stage::EditorEvents),
            _ => None,
        }
    }

    pub(super) fn phase(self) -> &'static str {
        self.stage().map_or("startup", Stage::name)
    }

    pub(super) fn active_stage(self) -> Option<Stage> {
        (self.0 & ACTIVE != 0).then(|| self.stage()).flatten()
    }
}

#[repr(C)]
pub(super) struct NativeOperation(AtomicU64);

impl NativeOperation {
    pub(super) const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        // One publication contains its generation, stage and active bit together.
        Snapshot(self.0.load(Ordering::Acquire))
    }

    /// Enter only on this lane's owner thread and without nesting another guard.
    pub(super) fn enter(&self, stage: Stage) -> Guard<'_> {
        // Each lane belongs to one fixed thread, including its startup and teardown.
        let previous = self.0.load(Ordering::Relaxed);
        let generation = (previous >> GENERATION_SHIFT).wrapping_add(1);
        let word = (generation << GENERATION_SHIFT) | ((stage as u64) << 1) | ACTIVE;
        // The parent observes the active call before native execution can stop making progress.
        self.0.store(word, Ordering::Release);
        Guard {
            operation: self,
            completed: word & !ACTIVE,
            _thread: PhantomData,
        }
    }
}

pub(super) struct Guard<'a> {
    operation: &'a NativeOperation,
    completed: u64,
    _thread: PhantomData<Rc<()>>,
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        // The lane's owner completes this guard before starting its next observed call.
        self.operation.0.store(self.completed, Ordering::Release);
    }
}

#[derive(Debug)]
pub(super) struct Watchdog {
    timeout: Duration,
    pending: Option<(Snapshot, Instant)>,
}

impl Watchdog {
    pub(super) const fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            pending: None,
        }
    }

    pub(super) fn reset(&mut self) {
        self.pending = None;
    }

    pub(super) fn expired(&mut self, snapshot: Snapshot, now: Instant) -> Option<Stage> {
        if snapshot.0 & ACTIVE == 0 {
            self.pending = None;
            return None;
        }
        let stage = snapshot.stage()?;
        match self.pending {
            Some((previous, since)) if previous == snapshot => {
                (now.duration_since(since) >= self.timeout).then_some(stage)
            }
            _ => {
                self.pending = Some((snapshot, now));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests;
