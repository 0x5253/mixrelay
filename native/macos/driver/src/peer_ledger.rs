//! Connection replay, incarnation fencing and publication settlements over opaque peers.
use crate::settlement::{Configured, Log, Since, Wanted};
use mixrelay_macos_transport::control_inventory::report::{Incompatible, Report};
use std::collections::BTreeMap;

pub(crate) trait PeerIdentity {
    fn identity(&self) -> usize;
}

/// How the HAL reported finishing one inventory generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Settlement {
    Applied,
    Rejected,
}

pub(crate) struct Peer<I> {
    pub(crate) object: I,
    pub(crate) subscribed: bool,
    pub(crate) settled: Option<(u64, Settlement)>,
    pub(crate) log: Log,
    pub(crate) incompatible: bool,
}

pub(crate) struct Peers<I> {
    pub(crate) values: Vec<Peer<I>>,
    pub(crate) replacements: BTreeMap<[u8; 16], u64>,
}
impl<I: PeerIdentity> Peers<I> {
    pub(crate) fn driver_connected(&self) -> bool {
        self.values.iter().any(|peer| peer.subscribed)
    }
    pub(crate) fn settlement(&self, generation: u64) -> Option<Settlement> {
        self.values
            .iter()
            .filter(|peer| peer.subscribed)
            .find_map(|peer| {
                peer.settled
                    .filter(|(settled, _)| *settled == generation)
                    .map(|(_, settlement)| settlement)
            })
    }
    fn subscribed(&self) -> impl DoubleEndedIterator<Item = &Peer<I>> {
        self.values.iter().filter(|peer| peer.subscribed)
    }
    /// The bound for a request issued now, from the newest subscribed HAL connection
    /// that finished its replay.
    pub(crate) fn since(&self) -> Option<Since> {
        self.subscribed().rev().find_map(|peer| peer.log.since())
    }
    /// The first settlement after `since` of a request for exactly `wanted`, read only
    /// from the newest connection of the incarnation `since` came from.
    pub(crate) fn settled(
        &self,
        endpoint: [u8; 16],
        since: Since,
        wanted: Wanted,
    ) -> Option<Configured> {
        self.subscribed()
            .rev()
            .find(|peer| peer.log.incarnation() == Some(since.incarnation))?
            .log
            .find(endpoint, since, wanted)
    }
    /// Drops connections older than `current` that report another incarnation once
    /// `current` has replayed: they belong to a HAL load that no longer runs.
    pub(crate) fn retire_before(&mut self, current: usize) {
        let Some(position) = self
            .values
            .iter()
            .position(|peer| peer.object.identity() == current)
        else {
            return;
        };
        let incarnation = self.values[position].log.incarnation();
        let mut index = 0;
        self.values.retain(|peer| {
            let older = index < position;
            index += 1;
            !older
                || peer
                    .log
                    .incarnation()
                    .is_none_or(|other| Some(other) == incarnation)
        });
    }
    /// Whether a subscribed HAL reports with another protocol version.
    pub(crate) fn incompatible(&self) -> bool {
        self.subscribed().any(|peer| peer.incompatible)
    }
}

impl<I> Default for Peers<I> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            replacements: BTreeMap::new(),
        }
    }
}

impl<I> Peer<I> {
    pub(crate) fn receive(
        &mut self,
        published_generation: Option<u64>,
        report: Result<Report, Incompatible>,
    ) {
        let Ok(report) = report else {
            self.incompatible = true;
            return;
        };
        match report {
            Report::Inventory {
                generation,
                applied,
            } => {
                let settlement = if applied {
                    Settlement::Applied
                } else {
                    Settlement::Rejected
                };
                // Never accept a settlement for anything but the current publication.
                if published_generation == Some(generation) {
                    self.settled = Some((generation, settlement));
                }
            }
            Report::Settled(configured) => self.log.record(configured),
            Report::Replayed { incarnation, .. } => self.log.replayed(incarnation),
            Report::Unprovisioned { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests;
