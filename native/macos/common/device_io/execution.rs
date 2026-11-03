//! Callback-owned entry/exit coordinates projected into the clock-break journal.
use std::cell::Cell;

#[derive(Debug, Default)]
pub(super) struct Execution {
    entered: Cell<u64>,
    recorded: Cell<bool>,
    previous: Cell<(u64, u64)>,
}

impl Execution {
    pub(super) fn enter<C: Fn() -> u64>(&self, entered: u64, finish: C) -> Span<'_, C> {
        self.entered.set(entered);
        self.recorded.set(false);
        Span {
            execution: self,
            entered,
            finish,
        }
    }

    pub(super) fn observe_clock(&self) {
        self.recorded.set(true);
    }

    pub(super) fn coordinates(&self) -> (u64, u64, u64) {
        let (previous_enter, previous_exit) = self.previous.get();
        (previous_enter, previous_exit, self.entered.get())
    }

    // Registration preparation owns this state only after the old IOProc drained.
    pub(super) fn reset(&self) {
        self.entered.set(0);
        self.recorded.set(false);
        self.previous.set((0, 0));
    }
}

pub(super) struct Span<'a, C: Fn() -> u64> {
    execution: &'a Execution,
    entered: u64,
    finish: C,
}

impl<C: Fn() -> u64> Drop for Span<'_, C> {
    fn drop(&mut self) {
        // Match Callback::previous_timestamp: early returns before record_clock
        // do not replace the last native packet's full execution coordinates.
        if self.execution.recorded.get() {
            self.execution.previous.set((self.entered, (self.finish)()));
        }
    }
}

#[cfg(test)]
mod tests;
