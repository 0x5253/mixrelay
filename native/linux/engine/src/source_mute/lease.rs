//! Forwarding leases and the rules that keep or revoke them against a registry snapshot.
use super::registry::Registry;
use mixrelay_linux_audio_io::process_identity::{ProcessIdentity, same_process};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// One playback stream muted at one owned endpoint, keyed by server serials.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct LeaseKey {
    pub(super) endpoint_serial: String,
    pub(super) source_serial: String,
}

#[derive(Clone, Debug)]
pub(super) struct Lease {
    pub(super) key: LeaseKey,
    pub(super) endpoint_id: u32,
    pub(super) source_id: u32,
    pub(super) endpoint_process: ProcessIdentity,
    pub(super) source_process: ProcessIdentity,
    pub(super) source_owner: ProcessIdentity,
    pub(super) endpoint_client_serial: String,
    pub(super) source_client_serial: String,
    pub(super) capture_healthy: Arc<AtomicBool>,
    pub(super) capture_fresh: Arc<AtomicBool>,
    pub(super) nonce: String,
    pub(super) references: usize,
    // Pinned when the lease is installed, not rebuilt from each desired plan.
    pub(super) other_sink_serials: BTreeSet<String>,
    pub(super) routing_target: Option<String>,
    pub(super) source_target_hint: Option<String>,
}

/// Installed leases for the active plan revision, plus keys revoked in that revision.
#[derive(Default)]
pub(super) struct LeaseState {
    pub(super) revision: u64,
    pub(super) leases: BTreeMap<LeaseKey, Lease>,
    pub(super) revoked: BTreeSet<LeaseKey>,
}

impl Lease {
    /// The capture tap is healthy and still delivering PCM, so muting loses nothing.
    pub(super) fn capture_ready(&self) -> bool {
        self.capture_healthy.load(Ordering::Acquire) && self.capture_fresh.load(Ordering::Acquire)
    }

    /// The metadata value `WirePlumber` must echo back; binds every pinned identity.
    pub(super) fn value(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{}",
            self.key.endpoint_serial,
            self.key.source_serial,
            self.source_process.process_id,
            self.source_process.start_time,
            self.source_owner.process_id,
            self.source_owner.start_time,
            self.endpoint_process.process_id,
            self.endpoint_process.start_time,
            self.nonce
        )
    }

    /// Both nodes still belong to the pinned processes and clients.
    pub(super) fn owners_unchanged(&self, registry: &Registry) -> bool {
        registry
            .source_owner(
                self.source_id,
                &self.key.source_serial,
                &self.source_process,
            )
            .is_some_and(|owner| same_process(&owner, &self.source_owner))
            && registry.verified_node(
                self.endpoint_id,
                &self.key.endpoint_serial,
                "Audio/Sink",
                &self.endpoint_process,
            )
            && registry.client_serial_for_node(self.endpoint_id).as_deref()
                == Some(self.endpoint_client_serial.as_str())
            && registry.client_serial_for_node(self.source_id).as_deref()
                == Some(self.source_client_serial.as_str())
    }

    /// Everything that authorized the lease still holds: live capture, the ACK, no
    /// link back to the endpoint, the pinned route, and the pinned owners.
    pub(super) fn still_valid(&self, registry: &Registry) -> bool {
        self.capture_ready()
            && registry.ack(self)
            && registry.links(self).is_empty()
            && self.route_unchanged(registry)
            && self.owners_unchanged(registry)
    }

    pub(super) fn route_unchanged(&self, registry: &Registry) -> bool {
        let Some(other_sink_serials) = registry.other_sink_serials(self) else {
            return false;
        };
        let Some(routing_target) = registry.routing_target(self.source_id) else {
            return false;
        };
        let Some(source_target_hint) = registry.source_target_hint(self.source_id) else {
            return false;
        };
        self.route_matches(&other_sink_serials, routing_target.as_deref())
            && source_target_hint.as_deref() == self.source_target_hint.as_deref()
    }

    /// The source plays to no sink beyond those pinned at install, and the policy
    /// target is unchanged. Losing a pinned sink is the user's change, not a reroute.
    pub(super) fn route_matches(
        &self,
        other_sink_serials: &BTreeSet<String>,
        routing_target: Option<&str>,
    ) -> bool {
        other_sink_serials.is_subset(&self.other_sink_serials)
            && routing_target == self.routing_target.as_deref()
    }

    /// The desired lease names the same nodes, processes and clients as this one.
    fn same_grant(&self, desired: &Self) -> bool {
        desired.endpoint_id == self.endpoint_id
            && desired.source_id == self.source_id
            && same_process(&desired.endpoint_process, &self.endpoint_process)
            && same_process(&desired.source_process, &self.source_process)
            && same_process(&desired.source_owner, &self.source_owner)
            && desired.endpoint_client_serial == self.endpoint_client_serial
            && desired.source_client_serial == self.source_client_serial
    }
}

impl LeaseState {
    /// Installed leases that fail `still_valid`.
    pub(super) fn invalid_keys(&self, still_valid: impl Fn(&Lease) -> bool) -> Vec<LeaseKey> {
        self.leases
            .iter()
            .filter_map(|(key, lease)| (!still_valid(lease)).then_some(key.clone()))
            .collect()
    }

    /// Installed leases the plan no longer wants, wants with a different grant, or
    /// that lost their authorization in `registry`.
    pub(super) fn stale_keys(
        &self,
        desired: &BTreeMap<LeaseKey, Lease>,
        registry: &Registry,
    ) -> Vec<LeaseKey> {
        self.invalid_keys(|current| {
            desired
                .get(&current.key)
                .is_some_and(|want| current.same_grant(want) && current.still_valid(registry))
        })
    }

    /// A new plan revision forgets revocations made under the previous one.
    pub(super) fn follow_revision(&mut self, revision: u64) {
        if self.revision != revision {
            self.revision = revision;
            self.revoked.clear();
        }
    }
}

#[cfg(test)]
pub(super) mod tests;
