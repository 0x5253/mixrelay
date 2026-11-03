//! HAL reports of how rate, format and mapping requests ended, and their attribution
//! to the request the service issued.
use std::collections::VecDeque;

/// Reports kept per HAL connection. Older entries predate every request still waiting:
/// a request waits at most `settle::ACK_TIMEOUT + settle::SETTLE_TIMEOUT`, and the HAL
/// settles far fewer requests than this in that time.
const RETAINED: usize = 256;

/// What one service request asked the HAL for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wanted {
    Rate(u32),
    /// A directional sample format, as a HAL `SampleFormat` code, at the device rate.
    Format {
        render: bool,
        rate: u32,
        format: u32,
    },
}

pub(crate) use mixrelay_macos_transport::control_inventory::report::Settled as Configured;

/// Whether `configured` ended a request for exactly `wanted` on `endpoint`.
fn answers(configured: &Configured, endpoint: [u8; 16], wanted: Wanted) -> bool {
    configured.endpoint == endpoint
        && match wanted {
            Wanted::Rate(rate) => configured.rate == rate,
            Wanted::Format {
                render,
                rate,
                format,
            } => configured.rate == rate && configured.formats[usize::from(render)] == format,
        }
}

/// The last settlement known when a request was issued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Since {
    pub(crate) incarnation: u64,
    pub(crate) sequence: u64,
}

/// One HAL connection's settlements, oldest first, bounded by `RETAINED`. A connection
/// first replays what the HAL retained; only after its marker is the newest sequence a
/// valid bound for a new request.
#[derive(Debug, Default)]
pub(crate) struct Log {
    incarnation: u64,
    replayed: bool,
    entries: VecDeque<Configured>,
}

impl Log {
    /// Starts over when the HAL reports from another incarnation.
    fn adopt(&mut self, incarnation: u64) {
        if self.incarnation != incarnation {
            *self = Self {
                incarnation,
                ..Self::default()
            };
        }
    }

    pub(crate) fn record(&mut self, configured: Configured) {
        self.adopt(configured.incarnation);
        // A reconnect replays retained settlements; keep each one once.
        if self.newest() >= configured.sequence {
            return;
        }
        if self.entries.len() == RETAINED {
            self.entries.pop_front();
        }
        self.entries.push_back(configured);
    }

    /// The HAL has resent every settlement it retained.
    pub(crate) fn replayed(&mut self, incarnation: u64) {
        self.adopt(incarnation);
        self.replayed = true;
    }

    fn newest(&self) -> u64 {
        self.entries
            .back()
            .map_or(0, |configured| configured.sequence)
    }

    /// The bound for a request issued now, once the replay has completed.
    pub(crate) fn since(&self) -> Option<Since> {
        self.replayed.then(|| Since {
            incarnation: self.incarnation,
            sequence: self.newest(),
        })
    }

    /// The HAL load this connection reports for, once it has reported.
    pub(crate) fn incarnation(&self) -> Option<u64> {
        (self.incarnation != 0).then_some(self.incarnation)
    }

    /// The first settlement after `since` that ended a request for exactly `wanted`.
    /// Sequences of another incarnation are unrelated to `since` and never match.
    pub(crate) fn find(
        &self,
        endpoint: [u8; 16],
        since: Since,
        wanted: Wanted,
    ) -> Option<Configured> {
        if self.incarnation != since.incarnation {
            return None;
        }
        self.entries
            .iter()
            .find(|configured| {
                configured.sequence > since.sequence && answers(configured, endpoint, wanted)
            })
            .copied()
    }
}

#[cfg(test)]
mod tests;
