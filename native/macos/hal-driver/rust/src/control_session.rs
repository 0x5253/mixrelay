//! Reconnect authority and heartbeat cadence owned by the Rust serial control queue.

const HEARTBEAT_TICKS: u32 = 10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
enum Phase {
    #[default]
    Disconnected = 0,
    AwaitingAuthority = 1,
    AwaitingSocket = 2,
    Connected = 3,
    Exhausted = 4,
}

/// Reconnect policy. The control owner keeps each borrow shorter than any host call.
#[derive(Debug, Default)]
pub(crate) struct ControlSession {
    generation: u64,
    phase: Phase,
    ticks: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum TickAction {
    #[default]
    Maintain = 0,
    RefreshInventory = 1,
    Reconnect = 2,
}

impl ControlSession {
    pub(crate) fn authority(&self) -> u64 {
        if self.accepts(self.generation) {
            self.generation
        } else {
            0
        }
    }

    pub(crate) fn begin(&mut self) -> u64 {
        let Some(next) = self.generation.checked_add(1) else {
            self.phase = Phase::Exhausted;
            return 0;
        };
        self.generation = next;
        self.phase = Phase::AwaitingAuthority;
        next
    }

    pub(crate) fn authorize(&mut self, generation: u64, accepted: bool) -> bool {
        if generation != self.generation || self.phase != Phase::AwaitingAuthority {
            return false;
        }
        self.phase = if accepted {
            Phase::AwaitingSocket
        } else {
            Phase::Disconnected
        };
        accepted
    }

    pub(crate) fn opened(&mut self, generation: u64, opened: bool) -> bool {
        if generation != self.generation || self.phase != Phase::AwaitingSocket {
            return false;
        }
        self.phase = if opened {
            Phase::Connected
        } else {
            Phase::Disconnected
        };
        opened
    }

    pub(crate) fn accepts(&self, generation: u64) -> bool {
        generation != 0 && self.generation == generation && self.phase == Phase::Connected
    }

    pub(crate) fn disconnected(&mut self, generation: u64) {
        if self.accepts(generation) {
            self.phase = Phase::Disconnected;
        }
    }

    pub(crate) fn tick(&mut self) -> TickAction {
        self.ticks += 1;
        if self.ticks < HEARTBEAT_TICKS {
            return TickAction::Maintain;
        }
        self.ticks = 0;
        match self.phase {
            Phase::Connected => TickAction::RefreshInventory,
            Phase::Disconnected | Phase::AwaitingAuthority | Phase::AwaitingSocket => {
                TickAction::Reconnect
            }
            Phase::Exhausted => TickAction::Maintain,
        }
    }
}

#[cfg(test)]
mod tests;
