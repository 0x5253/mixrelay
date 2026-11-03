//! Ephemeral latency authority is independent of persisted inventory revisions.
use crate::{DriverBackend, DriverError, LatencyGenerations};
use mixrelay_audio_io::VirtualEndpointId;
use mixrelay_time::Instant;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

/// Runtime latency route on the Driver service socket.
pub const LATENCY_ROUTE: &str = "/api/v1/driver/latency";
const LEASE_TIMEOUT: Duration = Duration::from_secs(3);

/// Service-issued authority; an engine incarnation cannot reuse a previous lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineLatencyLease {
    pub generation: u64,
    pub incarnation: String,
    pub process_id: u32,
}

/// Destination-rate frames for a validated common content arrival. `None` is unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLatency {
    pub endpoint_id: VirtualEndpointId,
    pub rate: u32,
    pub engine_lease: EngineLatencyLease,
    pub applied_graph_revision: u64,
    pub input_latency_frames: Option<u32>,
}

/// Control-only runtime publication; no audio callback performs HTTP.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum LatencyCommand {
    Claim {
        incarnation: String,
        process_id: u32,
    },
    /// The engine's complete set: an endpoint it no longer lists is unknown again.
    Publish {
        lease: EngineLatencyLease,
        /// Applied graph revision the set describes, also when it is empty. No entry may be
        /// newer. A set older than one the lease already sent is stale and ignored as a whole;
        /// otherwise the endpoints it omits are fenced at this revision.
        revision: u64,
        destinations: Vec<RuntimeLatency>,
    },
    Release {
        lease: EngineLatencyLease,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyReply {
    pub accepted: bool,
    pub lease: Option<EngineLatencyLease>,
}

pub(crate) struct LatencyAuthority {
    generations: Box<dyn LatencyGenerations>,
    current: Option<(EngineLatencyLease, Instant)>,
    /// What the HAL last accepted per endpoint.
    destinations: BTreeMap<VirtualEndpointId, RuntimeLatency>,
    /// Revision at which the current lease last omitted an endpoint. It stays until the
    /// endpoint is measured again, whether or not the HAL accepted the clear, so it fences
    /// older values and keeps a failed clear pending.
    omitted: BTreeMap<VirtualEndpointId, u64>,
    /// Revision of the newest set the current lease has published; an older set is stale.
    highest_revision: u64,
    generation: u64,
}
impl LatencyAuthority {
    pub(crate) fn new(generations: Box<dyn LatencyGenerations>) -> Self {
        Self {
            generations,
            current: None,
            destinations: BTreeMap::new(),
            omitted: BTreeMap::new(),
            highest_revision: 0,
            generation: 0,
        }
    }
    pub(crate) fn expire(
        &mut self,
        now: Instant,
        backend: &mut dyn DriverBackend,
    ) -> Result<(), DriverError> {
        if self
            .current
            .as_ref()
            .is_some_and(|(_, deadline)| now >= *deadline)
        {
            self.clear(backend)?;
        }
        Ok(())
    }
    /// Clears every endpoint, then ends the lease.
    /// # Errors
    /// Returns the first backend failure after attempting every endpoint. Failed endpoints
    /// and the lease stay, so the next expiry or release retries them.
    fn clear(&mut self, backend: &mut dyn DriverBackend) -> Result<(), DriverError> {
        let mut failure = None;
        self.destinations.retain(|_, publication| {
            let unknown = RuntimeLatency {
                input_latency_frames: None,
                ..publication.clone()
            };
            match backend.publish_latency(&unknown) {
                Ok(()) => false,
                Err(error) => {
                    failure.get_or_insert(error);
                    true
                }
            }
        });
        let destinations = &self.destinations;
        self.omitted
            .retain(|endpoint, _| destinations.contains_key(endpoint));
        if let Some(error) = failure {
            return Err(error);
        }
        self.current = None;
        Ok(())
    }
    pub(crate) fn command(
        &mut self,
        command: LatencyCommand,
        now: Instant,
        backend: &mut dyn DriverBackend,
    ) -> Result<LatencyReply, DriverError> {
        self.expire(now, backend)?;
        match command {
            LatencyCommand::Claim {
                incarnation,
                process_id,
            } => {
                if incarnation.is_empty()
                    || incarnation.len() > 128
                    || process_id == 0
                    || i32::try_from(process_id).is_err()
                {
                    return Err(DriverError::Invalid(
                        "invalid engine latency identity".into(),
                    ));
                }
                if self.current.is_some() {
                    return Ok(self.reply(false));
                }
                self.generation = self
                    .generation
                    .checked_add(1)
                    .ok_or_else(|| DriverError::Unavailable("latency lease exhausted".into()))?
                    .max(self.generations.candidate());
                self.highest_revision = 0;
                self.current = Some((
                    EngineLatencyLease {
                        generation: self.generation,
                        incarnation,
                        process_id,
                    },
                    now.saturating_add(LEASE_TIMEOUT),
                ));
            }
            LatencyCommand::Publish {
                lease,
                revision,
                destinations,
            } => {
                if !self.matches(&lease) {
                    return Ok(self.reply(false));
                }
                let listed = validated(&lease, revision, &destinations)?;
                // The engine is alive; a stale set or a backend failure does not end its lease.
                self.current = Some((lease, now.saturating_add(LEASE_TIMEOUT)));
                self.apply(revision, &listed, destinations, backend)?;
            }
            LatencyCommand::Release { lease } => {
                if !self.matches(&lease) {
                    return Ok(self.reply(false));
                }
                self.clear(backend)?;
            }
        }
        Ok(self.reply(true))
    }
    /// Applies the engine's validated complete set at `revision`. A set older than one the
    /// lease already published is stale and changes nothing.
    /// # Errors
    /// Every endpoint is attempted and the first backend failure is returned afterwards; a
    /// failed endpoint keeps its previous entry, so the next publication, release or expiry
    /// retries it.
    fn apply(
        &mut self,
        revision: u64,
        listed: &BTreeSet<VirtualEndpointId>,
        destinations: Vec<RuntimeLatency>,
        backend: &mut dyn DriverBackend,
    ) -> Result<(), DriverError> {
        if revision < self.highest_revision {
            return Ok(());
        }
        self.highest_revision = revision;
        self.omit(listed, revision);
        let mut failure = None;
        for entry in destinations {
            if self.fence(&entry.endpoint_id) > entry.applied_graph_revision {
                continue;
            }
            match backend.publish_latency(&entry) {
                Ok(()) => {
                    self.omitted.remove(&entry.endpoint_id);
                    self.destinations.insert(entry.endpoint_id.clone(), entry);
                }
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        if let Err(error) = self.clear_omitted(backend) {
            failure.get_or_insert(error);
        }
        failure.map_or(Ok(()), Err)
    }
    /// A route that no longer reaches an endpoint leaves no measurement for it: the endpoint
    /// is fenced at the omitting publication's revision, so a delayed value from an earlier
    /// revision cannot become measured again.
    fn omit(&mut self, listed: &BTreeSet<VirtualEndpointId>, revision: u64) {
        for endpoint in self.destinations.keys() {
            if !listed.contains(endpoint) {
                let fence = self.omitted.entry(endpoint.clone()).or_default();
                *fence = (*fence).max(revision);
            }
        }
    }
    /// The oldest revision whose value the endpoint may still accept.
    fn fence(&self, endpoint: &VirtualEndpointId) -> u64 {
        let accepted = self
            .destinations
            .get(endpoint)
            .map_or(0, |entry| entry.applied_graph_revision);
        self.omitted
            .get(endpoint)
            .map_or(accepted, |omitted| accepted.max(*omitted))
    }
    /// Sends unknown at its omission revision for every omitted endpoint the HAL still
    /// reports as measured.
    /// # Errors
    /// Returns the first backend failure after attempting every endpoint. A failed endpoint
    /// keeps its measured entry so the next publication, release or expiry retries it.
    fn clear_omitted(&mut self, backend: &mut dyn DriverBackend) -> Result<(), DriverError> {
        let mut failure = None;
        for (endpoint, revision) in &self.omitted {
            let Some(old) = self.destinations.get_mut(endpoint) else {
                continue;
            };
            if old.input_latency_frames.is_none() {
                continue;
            }
            let unknown = RuntimeLatency {
                applied_graph_revision: *revision,
                input_latency_frames: None,
                ..old.clone()
            };
            match backend.publish_latency(&unknown) {
                Ok(()) => *old = unknown,
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
    fn matches(&self, lease: &EngineLatencyLease) -> bool {
        self.current
            .as_ref()
            .is_some_and(|(current, _)| current == lease)
    }
    fn reply(&self, accepted: bool) -> LatencyReply {
        LatencyReply {
            accepted,
            lease: self.current.as_ref().map(|(lease, _)| lease.clone()),
        }
    }
}

/// The endpoints a complete set lists.
/// # Errors
/// Rejects oversized sets, zero rates, entries of another lease or newer than the set's
/// revision, and duplicate endpoints.
fn validated(
    lease: &EngineLatencyLease,
    revision: u64,
    destinations: &[RuntimeLatency],
) -> Result<BTreeSet<VirtualEndpointId>, DriverError> {
    if destinations.len() > 64
        || destinations.iter().any(|entry| {
            entry.rate == 0
                || entry.engine_lease != *lease
                || entry.applied_graph_revision > revision
        })
    {
        return Err(DriverError::Invalid("invalid latency publication".into()));
    }
    let mut listed = BTreeSet::new();
    if destinations
        .iter()
        .any(|entry| !listed.insert(entry.endpoint_id.clone()))
    {
        return Err(DriverError::Invalid("duplicate latency endpoint".into()));
    }
    Ok(listed)
}

#[cfg(test)]
mod tests;
