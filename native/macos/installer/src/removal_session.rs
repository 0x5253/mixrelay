//! Removal replies retain their signed sender until its caller finishes or abandons it.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Event {
    Admission,
    Transaction(i32),
    Acknowledged,
    Disconnected,
    Abandoned,
    IdleDeadline,
    Terminated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(crate) enum Action {
    Ignore = 0,
    AllowRemoval = 1,
    AwaitAcknowledgement = 2,
    Finish = 4,
    Idle = 8,
    AwaitIdle = 16,
}

#[derive(Debug, Default)]
pub(crate) struct RemovalSession {
    committed_peer: Option<usize>,
    stopping: bool,
}

impl RemovalSession {
    pub(crate) fn event(&mut self, event: Event, peer: usize) -> Action {
        if self.stopping {
            return Action::Ignore;
        }
        match event {
            Event::Admission if self.committed_peer.is_none() => Action::AllowRemoval,
            Event::Transaction(status) if self.committed_peer.is_none() => {
                if status == 0 || status == 2 {
                    self.committed_peer = Some(peer);
                    Action::AwaitAcknowledgement
                } else {
                    Action::AwaitIdle
                }
            }
            Event::Acknowledged | Event::Disconnected if self.committed_peer == Some(peer) => {
                self.stopping = true;
                Action::Finish
            }
            Event::Abandoned if self.committed_peer.is_some() => {
                self.stopping = true;
                Action::Finish
            }
            Event::IdleDeadline if self.committed_peer.is_none() => {
                self.stopping = true;
                Action::Idle
            }
            Event::Terminated => {
                self.stopping = true;
                Action::Idle
            }
            _ => Action::Ignore,
        }
    }
}

#[cfg(test)]
mod tests;
