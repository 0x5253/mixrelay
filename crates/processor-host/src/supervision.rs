//! Child supervision decisions over progress observations and explicit monotonic time.

use std::time::Duration;

use mixrelay_time::{Deadline, Instant};

use crate::native_operation::{Snapshot, Stage, Watchdog};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChildState {
    Running,
    Exited,
    WaitFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StopStatus {
    Running,
    Requested,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct NativeActivity {
    pub(super) creator: Snapshot,
    pub(super) dsp: Snapshot,
    pub(super) parent_request_pending: bool,
}

impl NativeActivity {
    pub(super) fn phase(self) -> &'static str {
        self.creator
            .active_stage()
            .or_else(|| self.dsp.active_stage())
            .map_or("worker", Stage::name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    ChildWait,
    NativeTimeout(Stage),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Continue,
    Finished { unexpected: bool },
    Kill { failure: Option<Failure> },
}

#[derive(Debug)]
pub(super) struct Supervisor {
    creator: Watchdog,
    dsp: Watchdog,
    shutdown_grace: Duration,
    stop_deadline: Option<Deadline>,
    next_timing_sample: Instant,
}

impl Supervisor {
    pub(super) fn new(native_timeout: Duration, shutdown_grace: Duration, now: Instant) -> Self {
        Self {
            creator: Watchdog::new(native_timeout),
            dsp: Watchdog::new(native_timeout),
            shutdown_grace,
            stop_deadline: None,
            next_timing_sample: now,
        }
    }

    pub(super) fn sample_due(&mut self, now: Instant) -> bool {
        if now < self.next_timing_sample {
            return false;
        }
        self.next_timing_sample = now.saturating_add(Duration::from_secs(1));
        true
    }

    pub(super) fn observe(
        &mut self,
        child: ChildState,
        stop: StopStatus,
        activity: NativeActivity,
        now: Instant,
    ) -> Action {
        match child {
            ChildState::Exited => {
                return Action::Finished {
                    unexpected: stop == StopStatus::Running,
                };
            }
            ChildState::WaitFailed => {
                return Action::Kill {
                    failure: Some(Failure::ChildWait),
                };
            }
            ChildState::Running => {}
        }
        if let Some(stage) = self.native_timeout(activity, now) {
            return Action::Kill {
                failure: Some(Failure::NativeTimeout(stage)),
            };
        }
        if stop == StopStatus::Requested {
            let deadline = self
                .stop_deadline
                .get_or_insert_with(|| Deadline::after(now, self.shutdown_grace));
            if deadline.has_passed(now) {
                return Action::Kill { failure: None };
            }
        }
        Action::Continue
    }

    fn native_timeout(&mut self, activity: NativeActivity, now: Instant) -> Option<Stage> {
        // Mouse tracking can wait on the user. Its deadline begins only when the parent
        // requests a response; an interactive creator never exempts the DSP lane.
        let creator_timeout = if activity.creator.active_stage() == Some(Stage::EditorEvents)
            && !activity.parent_request_pending
        {
            self.creator.reset();
            None
        } else {
            self.creator.expired(activity.creator, now)
        };
        creator_timeout.or_else(|| self.dsp.expired(activity.dsp, now))
    }
}

#[cfg(test)]
mod tests;
