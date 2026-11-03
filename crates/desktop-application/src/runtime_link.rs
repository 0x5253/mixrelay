//! Multiplexed route admission and terminal command-frame interpretation.
use mixrelay_time::Instant;
use serde_json::Value;
use std::{collections::HashMap, time::Duration};

/// A connection whose last active route ended may retire before accepting a new one.
pub struct RouteTable<R> {
    live: HashMap<u64, R>,
    closed: bool,
    idle_since: Option<Instant>,
}

/// Why a route cannot be admitted to this connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionFailure {
    Closed,
    IdleExpired,
}
impl<R> RouteTable<R> {
    /// Starts with no active routes and a supplied idle origin.
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            live: HashMap::new(),
            closed: false,
            idle_since: Some(now),
        }
    }
    /// Registers only while the connection is live and inside its idle reuse window.
    /// # Errors
    /// Rejects closed links and expired idle reuse windows.
    pub fn register(
        &mut self,
        id: u64,
        route: R,
        now: Instant,
        reuse: Duration,
    ) -> Result<(), AdmissionFailure> {
        if self.closed {
            return Err(AdmissionFailure::Closed);
        }
        if self
            .idle_since
            .is_some_and(|since| now.duration_since(since) >= reuse)
        {
            return Err(AdmissionFailure::IdleExpired);
        }
        self.live.insert(id, route);
        self.idle_since = None;
        Ok(())
    }
    /// Looks up a route for delivery of an already admitted frame.
    pub fn get_mut(&mut self, id: u64) -> Option<&mut R> {
        self.live.get_mut(&id)
    }
    /// Ends one route and starts the reuse window when the final route ends.
    pub fn remove(&mut self, id: u64, now: Instant) {
        self.live.remove(&id);
        if self.live.is_empty() && self.idle_since.is_none() {
            self.idle_since = Some(now);
        }
    }
    /// Ends admission and returns every remaining route for adapter cleanup.
    pub fn close(&mut self) -> HashMap<u64, R> {
        self.closed = true;
        std::mem::take(&mut self.live)
    }
    /// Whether connection cleanup has closed admission.
    #[must_use]
    pub fn closed(&self) -> bool {
        self.closed
    }
}

/// A command heartbeat is observation only; terminal service errors remain service errors.
#[derive(Debug, PartialEq)]
pub enum CommandFrame {
    Heartbeat,
    Complete(Result<Value, Value>),
    ControllerUnavailable,
    InvalidResponse,
}

/// Interprets one tagged command frame after the transport selected its route.
#[must_use]
pub fn command_frame(mut reply: Value) -> CommandFrame {
    if reply["status"] == 102 {
        return CommandFrame::Heartbeat;
    }
    if reply["status"] != 200 {
        return CommandFrame::ControllerUnavailable;
    }
    if let Some(value) = reply["body"].get_mut("ok") {
        return CommandFrame::Complete(Ok(value.take()));
    }
    if let Some(error) = reply["body"].get_mut("error") {
        return CommandFrame::Complete(Err(error.take()));
    }
    CommandFrame::InvalidResponse
}

#[cfg(test)]
mod tests;
