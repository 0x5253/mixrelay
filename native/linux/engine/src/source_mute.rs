//! Exact, off-RT `PipeWire` registry and `WirePlumber` ACK for a scoped forwarding lease.
//! No selector string, display name, or client-supplied PID authorizes link removal.
use crate::{applications::CaptureWatch, endpoints::EndpointSpecs};
use commands::PipeWireSession;
use lease::{Lease, LeaseKey, LeaseState};
use mixrelay_audio_graph::composition::ApplicationPlaybackIntent;
use mixrelay_audio_io::HostAudioBackendError;
use mixrelay_engine_core::PlaybackBindings;
use mixrelay_render::{ProcessorExchange, ProcessorRevisions, RuntimeControlBank};
use mixrelay_time::{MonotonicClock, Sleeper};
use registry::Registry;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

mod commands;
mod desired;
mod lease;
mod registry;

#[derive(Clone)]
pub(crate) enum OwnedPlan {
    Preserve,
    Forward {
        intents: Vec<ApplicationPlaybackIntent>,
        controls: Arc<RuntimeControlBank>,
    },
}

/// Control-only view of the active native owners and negotiated render format.
pub(crate) struct PlaybackResources<'a> {
    pub(crate) watch: Option<&'a CaptureWatch>,
    pub(crate) endpoints: &'a EndpointSpecs,
    pub(crate) driver: Option<&'a mixrelay_driver_core::DriverSnapshot>,
}

/// Keeps `WirePlumber` forwarding leases in step with the plan the audio thread runs.
pub(crate) struct PlaybackController {
    plans: Mutex<BTreeMap<u64, OwnedPlan>>,
    state: Mutex<LeaseState>,
    session: PipeWireSession,
}

impl PlaybackController {
    /// Drives the installed `PipeWire` tools, with deadlines and polls timed by `timer`.
    pub(crate) fn installed(timer: impl MonotonicClock + Sleeper + 'static) -> Self {
        Self::new(PipeWireSession::installed(timer))
    }

    fn new(session: PipeWireSession) -> Self {
        Self {
            plans: Mutex::default(),
            state: Mutex::default(),
            session,
        }
    }

    /// Owns the plan's playback bindings. Forwarding is accepted only while the
    /// `WirePlumber` hook that honours leases is loaded.
    pub(crate) fn prepare_plan(
        &self,
        playback: &PlaybackBindings<'_>,
    ) -> Result<OwnedPlan, HostAudioBackendError> {
        match playback {
            PlaybackBindings::Preserve => Ok(OwnedPlan::Preserve),
            PlaybackBindings::Forward { intents, controls } => {
                let ready = self
                    .session
                    .read_registry()
                    .map_err(capability_unavailable)?
                    .hook_ready();
                if !ready {
                    return Err(capability_unavailable(
                        "WirePlumber 0.5 MixRelay source-mute hook is not loaded".into(),
                    ));
                }
                Ok(OwnedPlan::Forward {
                    intents: (*intents).to_vec(),
                    controls: Arc::clone(controls),
                })
            }
        }
    }

    pub(crate) fn stage(&self, revision: u64, plan: OwnedPlan) {
        lock(&self.plans).insert(revision, plan);
    }

    /// Control-worker reconciliation; never called from the `PipeWire` callback.
    pub(crate) fn poll(&self, exchange: &ProcessorExchange, resources: &PlaybackResources<'_>) {
        let revisions = exchange.plan_revisions();
        let plan = self.active_plan(revisions);
        let mut state = lock(&self.state);
        state.follow_revision(revisions.active);
        if matches!(plan, OwnedPlan::Preserve) && state.leases.is_empty() {
            return;
        }
        let registry = match self.session.read_registry() {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "could not inspect source-mute leases");
                return;
            }
        };
        if revisions.submitted != revisions.active {
            // Newly staged watchers/endpoints cannot authorize an old active lease.
            let invalid = state.invalid_keys(|lease| lease.still_valid(&registry));
            self.revoke(
                &mut state,
                invalid,
                "could not release pending-transition source-mute lease",
            );
            return;
        }
        let desired =
            desired::desired_leases(&plan, resources, &registry).unwrap_or_else(|error| {
                tracing::warn!(%error, "source-mute capability no longer available");
                BTreeMap::new()
            });
        self.release_stale(&mut state, &desired, &registry);
        self.install_missing(&mut state, desired, exchange, revisions.active);
    }

    /// The plan the audio thread runs now. Plans for neither the active nor the
    /// submitted revision are dropped; an unknown revision preserves playback.
    fn active_plan(&self, revisions: ProcessorRevisions) -> OwnedPlan {
        let mut plans = lock(&self.plans);
        plans.retain(|revision, _| {
            *revision == revisions.active || *revision == revisions.submitted
        });
        plans
            .get(&revisions.active)
            .cloned()
            .unwrap_or(OwnedPlan::Preserve)
    }

    /// Clears each lease and marks it revoked for this revision. A lease whose
    /// metadata cannot be cleared stays installed so a later poll retries.
    fn revoke(&self, state: &mut LeaseState, keys: Vec<LeaseKey>, failure: &str) {
        for key in keys {
            let Some(lease) = state.leases.get(&key) else {
                continue;
            };
            if let Err(error) = self.session.metadata_clear(lease) {
                tracing::warn!(%error, "{failure}");
                continue;
            }
            state.leases.remove(&key);
            state.revoked.insert(key);
        }
    }

    /// Clears leases the plan no longer authorizes. A lease still wanted but rerouted
    /// by someone else is revoked, so this revision never reinstalls it.
    fn release_stale(
        &self,
        state: &mut LeaseState,
        desired: &BTreeMap<LeaseKey, Lease>,
        registry: &Registry,
    ) {
        for key in state.stale_keys(desired, registry) {
            let Some(current) = state.leases.get(&key) else {
                continue;
            };
            let external_link =
                !registry.links(current).is_empty() || !current.route_unchanged(registry);
            if let Err(error) = self.session.metadata_clear(current) {
                tracing::warn!(%error, "could not release source-mute lease");
                continue;
            }
            state.leases.remove(&key);
            if external_link && desired.contains_key(&key) {
                state.revoked.insert(key.clone());
                tracing::warn!(
                    ?key,
                    "external forwarding reroute revoked source-mute lease"
                );
            }
        }
    }

    /// Installs wanted leases that are neither installed nor revoked, abandoning any
    /// whose install races a plan change.
    fn install_missing(
        &self,
        state: &mut LeaseState,
        desired: BTreeMap<LeaseKey, Lease>,
        exchange: &ProcessorExchange,
        revision: u64,
    ) {
        for (key, mut want) in desired {
            if let Some(existing) = state.leases.get_mut(&key) {
                existing.references = want.references;
                continue;
            }
            if state.revoked.contains(&key) || exchange.plan_revisions().active != revision {
                continue;
            }
            match commands::new_nonce() {
                Ok(nonce) => want.nonce = nonce,
                Err(error) => {
                    tracing::warn!(%error, "source-mute nonce unavailable");
                    continue;
                }
            }
            if let Err(error) = self.session.install(&mut want) {
                tracing::warn!(%error, ?key, "source-mute ACK or scoped link removal failed");
                state.revoked.insert(key);
                continue;
            }
            if exchange.plan_revisions().active != revision {
                let _ = self.session.metadata_clear(&want);
                continue;
            }
            state.leases.insert(key, want);
        }
    }

    pub(crate) fn clear_all(&self) {
        let mut state = lock(&self.state);
        for lease in state.leases.values() {
            if let Err(error) = self.session.metadata_clear(lease) {
                tracing::warn!(%error, "source-mute shutdown lease cleanup failed");
            }
        }
        state.leases.clear();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn capability_unavailable(capability: String) -> HostAudioBackendError {
    HostAudioBackendError::CapabilityUnavailable { capability }
}
