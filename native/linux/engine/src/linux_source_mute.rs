//! Exact, off-RT `PipeWire` registry and `WirePlumber` ACK for a scoped forwarding lease.
//! No selector string, display name, or client-supplied PID authorizes link removal.
use super::{CaptureWatch, EndpointSpecs};
use mixrelay_audio_graph::composition::ApplicationPlaybackIntent;
use mixrelay_engine_core::PlaybackBindings;
use mixrelay_linux_audio_io::process_identity::{ProcessIdentity, process_identity, same_process};
use mixrelay_render::{ProcessorExchange, RuntimeControlBank};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const METADATA_NAME: &str = "mixrelay-source-mute";

#[derive(Debug, PartialEq, Eq)]
enum ObservedTarget {
    Unset,
    Target(String),
}

impl ObservedTarget {
    fn as_deref(&self) -> Option<&str> {
        match self {
            Self::Unset => None,
            Self::Target(value) => Some(value),
        }
    }

    fn into_option(self) -> Option<String> {
        match self {
            Self::Unset => None,
            Self::Target(value) => Some(value),
        }
    }
}

#[derive(Clone)]
pub(super) enum OwnedPlan {
    Preserve,
    Forward {
        intents: Vec<ApplicationPlaybackIntent>,
        controls: Arc<RuntimeControlBank>,
    },
}

impl OwnedPlan {
    pub(super) fn prepare(playback: &PlaybackBindings<'_>) -> Result<Self, String> {
        match playback {
            PlaybackBindings::Preserve => Ok(Self::Preserve),
            PlaybackBindings::Forward { intents, controls } => {
                if !Registry::read()?.hook_ready() {
                    return Err("WirePlumber 0.5 MixRelay source-mute hook is not loaded".into());
                }
                Ok(Self::Forward {
                    intents: (*intents).to_vec(),
                    controls: Arc::clone(controls),
                })
            }
        }
    }
}

/// Control-only view of the active native owners and negotiated render format.
pub(super) struct PlaybackResources<'a> {
    pub watch: Option<&'a CaptureWatch>,
    pub endpoints: &'a EndpointSpecs,
    pub driver: Option<&'a mixrelay_driver_core::DriverSnapshot>,
}

#[derive(Default)]
pub(super) struct PlaybackController {
    plans: Mutex<BTreeMap<u64, OwnedPlan>>,
    state: Mutex<LeaseState>,
}

#[derive(Default)]
struct LeaseState {
    revision: u64,
    leases: BTreeMap<LeaseKey, Lease>,
    revoked: BTreeSet<LeaseKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct LeaseKey {
    pub endpoint_serial: String,
    pub source_serial: String,
}

#[derive(Clone, Debug)]
pub(super) struct Lease {
    pub key: LeaseKey,
    pub endpoint_id: u32,
    pub source_id: u32,
    pub endpoint_process: ProcessIdentity,
    pub source_process: ProcessIdentity,
    pub source_owner: ProcessIdentity,
    pub endpoint_client_serial: String,
    pub source_client_serial: String,
    pub capture_healthy: Arc<std::sync::atomic::AtomicBool>,
    pub capture_fresh: Arc<std::sync::atomic::AtomicBool>,
    pub nonce: String,
    pub references: usize,
    // Pinned when the lease is installed, not rebuilt from each desired plan.
    pub other_sink_serials: BTreeSet<String>,
    pub routing_target: Option<String>,
    pub source_target_hint: Option<String>,
}

impl Lease {
    fn capture_ready(&self) -> bool {
        self.capture_healthy
            .load(std::sync::atomic::Ordering::Acquire)
            && self
                .capture_fresh
                .load(std::sync::atomic::Ordering::Acquire)
    }

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
}

fn lease_still_valid(registry: &Registry, lease: &Lease) -> bool {
    lease.capture_ready()
        && registry.ack(lease)
        && registry.links(lease).is_empty()
        && route_unchanged(registry, lease)
        && registry.verified_node(
            lease.endpoint_id,
            &lease.key.endpoint_serial,
            "Audio/Sink",
            &lease.endpoint_process,
        )
        && registry
            .source_owner(
                lease.source_id,
                &lease.key.source_serial,
                &lease.source_process,
            )
            .is_some_and(|owner| same_process(&owner, &lease.source_owner))
        && registry
            .client_serial_for_node(lease.endpoint_id)
            .as_deref()
            == Some(lease.endpoint_client_serial.as_str())
        && registry.client_serial_for_node(lease.source_id).as_deref()
            == Some(lease.source_client_serial.as_str())
}

fn route_unchanged(registry: &Registry, lease: &Lease) -> bool {
    let Some(other_sink_serials) = registry.other_sink_serials(lease) else {
        return false;
    };
    let Some(routing_target) = registry.routing_target(lease.source_id) else {
        return false;
    };
    let Some(source_target_hint) = registry.source_target_hint(lease.source_id) else {
        return false;
    };
    route_matches(lease, &other_sink_serials, routing_target.as_deref())
        && source_target_hint.as_deref() == lease.source_target_hint.as_deref()
}

fn route_matches(
    lease: &Lease,
    other_sink_serials: &BTreeSet<String>,
    routing_target: Option<&str>,
) -> bool {
    other_sink_serials.is_subset(&lease.other_sink_serials)
        && routing_target == lease.routing_target.as_deref()
}

fn pending_transition_stale_keys(
    state: &LeaseState,
    still_valid: impl Fn(&Lease) -> bool,
) -> Vec<LeaseKey> {
    state
        .leases
        .iter()
        .filter_map(|(key, lease)| (!still_valid(lease)).then_some(key.clone()))
        .collect()
}

fn audit_pending_leases(state: &mut LeaseState, registry: &Registry) {
    // Newly staged watchers/endpoints cannot authorize an old active lease.
    let expired_keys =
        pending_transition_stale_keys(state, |lease| lease_still_valid(registry, lease));
    for key in expired_keys {
        let Some(lease) = state.leases.get(&key) else {
            continue;
        };
        if let Err(error) = metadata_clear(lease) {
            tracing::warn!(%error, "could not release pending-transition source-mute lease");
            continue;
        }
        state.leases.remove(&key);
        state.revoked.insert(key);
    }
}

fn release_stale_leases(
    state: &mut LeaseState,
    desired: &BTreeMap<LeaseKey, Lease>,
    registry: &Registry,
) {
    let expired_keys = state
        .leases
        .iter()
        .filter_map(|(key, current)| {
            let matching = desired.get(key).is_some_and(|want| {
                want.endpoint_id == current.endpoint_id
                    && want.source_id == current.source_id
                    && same_process(&want.endpoint_process, &current.endpoint_process)
                    && same_process(&want.source_process, &current.source_process)
                    && same_process(&want.source_owner, &current.source_owner)
                    && want.endpoint_client_serial == current.endpoint_client_serial
                    && want.source_client_serial == current.source_client_serial
                    && lease_still_valid(registry, current)
            });
            (!matching).then_some(key.clone())
        })
        .collect::<Vec<_>>();
    for key in expired_keys {
        let Some(current) = state.leases.get(&key) else {
            continue;
        };
        let external_link =
            !registry.links(current).is_empty() || !route_unchanged(registry, current);
        if let Err(error) = metadata_clear(current) {
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

fn owned_render(
    intent: &ApplicationPlaybackIntent,
    resources: &PlaybackResources<'_>,
    registry: &Registry,
    driver_owner: &ProcessIdentity,
) -> Option<(u32, String, String)> {
    if !resources
        .endpoints
        .values()
        .any(|spec| spec.id == intent.forwarding_endpoint)
    {
        return None;
    }
    let device = resources.driver.and_then(|snapshot| {
        snapshot
            .settings
            .devices
            .iter()
            .find(|device| device.spec.id == intent.forwarding_endpoint)
    })?;
    if device.spec.render.is_none() || device.native_formats.render.current.is_none() {
        return None;
    }
    let expected_name = format!("{}.render", super::endpoints::native_name(&device.spec.id));
    let mut candidates = registry.0.keys().filter_map(|&id| {
        registry
            .endpoint(id, &expected_name, driver_owner)
            .map(|serial| (id, serial))
    });
    let (endpoint_id, endpoint_serial) = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    registry
        .client_serial_for_node(endpoint_id)
        .map(|client_serial| (endpoint_id, endpoint_serial, client_serial))
}

fn desired_leases(
    plan: &OwnedPlan,
    resources: &PlaybackResources<'_>,
    registry: &Registry,
) -> Result<BTreeMap<LeaseKey, Lease>, String> {
    let OwnedPlan::Forward {
        intents,
        controls: bank,
    } = plan
    else {
        return Ok(BTreeMap::new());
    };
    if !registry.hook_ready() {
        return Err("WirePlumber source-mute hook disappeared".into());
    }
    let Some(watch) = resources.watch else {
        return Ok(BTreeMap::new());
    };
    let inventory = mixrelay_linux_audio_io::targets().map_err(|e| e.to_string())?;
    let driver = resources
        .driver
        .ok_or("Driver owner snapshot unavailable")?;
    if driver.state != mixrelay_driver_core::DriverState::Ready {
        return Err("Driver is not ready".into());
    }
    let driver_owner = driver
        .owner_process_id
        .and_then(process_identity)
        .ok_or("Driver process identity unavailable")?;
    let mut desired: BTreeMap<LeaseKey, Lease> = BTreeMap::new();
    for intent in intents {
        if !bank
            .snapshot(&intent.mask_node_id)
            .is_some_and(|(control, _)| control.enabled)
        {
            continue;
        }
        let Some((endpoint_id, endpoint_serial, endpoint_client_serial)) =
            owned_render(intent, resources, registry, &driver_owner)
        else {
            continue;
        };
        for target in watch.verified_playback_targets(intent.capture_node_id.as_str(), &inventory) {
            let Some(source_owner) =
                registry.source_owner(target.object_id, &target.serial, &target.process)
            else {
                continue;
            };
            let Some(source_client_serial) = registry.client_serial_for_node(target.object_id)
            else {
                continue;
            };
            let key = LeaseKey {
                endpoint_serial: endpoint_serial.clone(),
                source_serial: target.serial,
            };
            if let Some(existing) = desired.get_mut(&key) {
                existing.references += 1;
            } else {
                desired.insert(
                    key.clone(),
                    Lease {
                        key,
                        endpoint_id,
                        source_id: target.object_id,
                        endpoint_process: driver_owner.clone(),
                        source_process: target.process,
                        source_owner,
                        endpoint_client_serial: endpoint_client_serial.clone(),
                        source_client_serial,
                        capture_healthy: target.healthy,
                        capture_fresh: target.fresh_pcm,
                        nonce: String::new(),
                        references: 1,
                        other_sink_serials: BTreeSet::new(),
                        routing_target: None,
                        source_target_hint: None,
                    },
                );
            }
        }
    }
    Ok(desired)
}

impl PlaybackController {
    pub(super) fn stage(&self, revision: u64, plan: OwnedPlan) {
        self.plans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(revision, plan);
    }

    /// Control-worker reconciliation; never called from the `PipeWire` callback.
    pub(super) fn poll(&self, exchange: &ProcessorExchange, resources: &PlaybackResources<'_>) {
        let revisions = exchange.plan_revisions();
        let plan = {
            let mut plans = self
                .plans
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            plans.retain(|revision, _| {
                *revision == revisions.active || *revision == revisions.submitted
            });
            plans
                .get(&revisions.active)
                .cloned()
                .unwrap_or(OwnedPlan::Preserve)
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.revision != revisions.active {
            state.revision = revisions.active;
            state.revoked.clear();
        }
        if matches!(plan, OwnedPlan::Preserve) && state.leases.is_empty() {
            return;
        }
        let registry = match Registry::read() {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "could not inspect source-mute leases");
                return;
            }
        };
        if revisions.submitted != revisions.active {
            audit_pending_leases(&mut state, &registry);
            return;
        }
        let desired = match desired_leases(&plan, resources, &registry) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "source-mute capability no longer available");
                BTreeMap::new()
            }
        };
        release_stale_leases(&mut state, &desired, &registry);
        for (key, mut want) in desired {
            if let Some(existing) = state.leases.get_mut(&key) {
                existing.references = want.references;
                continue;
            }
            if state.revoked.contains(&key) || exchange.plan_revisions().active != revisions.active
            {
                continue;
            }
            match new_nonce() {
                Ok(nonce) => want.nonce = nonce,
                Err(error) => {
                    tracing::warn!(%error, "source-mute nonce unavailable");
                    continue;
                }
            }
            if let Err(error) = install(&mut want) {
                tracing::warn!(%error, ?key, "source-mute ACK or scoped link removal failed");
                state.revoked.insert(key);
                continue;
            }
            if exchange.plan_revisions().active != revisions.active {
                let _ = metadata_clear(&want);
                continue;
            }
            state.leases.insert(key, want);
        }
    }

    pub(super) fn clear_all(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for lease in state.leases.values() {
            if let Err(error) = metadata_clear(lease) {
                tracing::warn!(%error, "source-mute shutdown lease cleanup failed");
            }
        }
        state.leases.clear();
    }
}

pub(super) struct Registry(BTreeMap<u32, Value>);

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

fn number(value: &Value, key: &str) -> Option<u32> {
    let value = value.get(key)?;
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .or_else(|| value.as_str()?.parse().ok())
}

fn serial(value: &Value) -> Option<String> {
    value
        .as_u64()
        .map(|n| n.to_string())
        .or_else(|| value.as_str().map(str::to_owned))
}

fn routing_target_value(metadata: &Value, source_id: u32) -> Option<ObservedTarget> {
    let entries = metadata.get("metadata")?.as_array()?;
    let target = entries.iter().find(|item| {
        number(item, "subject") == Some(source_id) && field(item, "key") == Some("target.object")
    });
    match target {
        None => Some(ObservedTarget::Unset),
        Some(item) => match item.get("value") {
            Some(Value::Null) => Some(ObservedTarget::Unset),
            Some(Value::String(value)) => Some(ObservedTarget::Target(value.clone())),
            _ => None,
        },
    }
}

fn merge(current: &mut Value, update: Value) {
    match (current, update) {
        (Value::Object(old), Value::Object(new)) => {
            for (key, value) in new {
                merge(old.entry(key).or_insert(Value::Null), value);
            }
        }
        (current, update) => *current = update,
    }
}

fn pulse_claim_matches(
    owner: &ProcessIdentity,
    selected: &ProcessIdentity,
    node: &Value,
    client: &Value,
) -> bool {
    let Some(executable) = selected.executable_path.as_deref() else {
        return false;
    };
    let Some(binary) = std::path::Path::new(executable)
        .file_name()
        .and_then(|s| s.to_str())
    else {
        return false;
    };
    let reported_binary_matches = |props: &Value| {
        field(props, "application.process.binary")
            .is_some_and(|reported| reported == binary || reported == executable)
    };
    owner
        .executable_path
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_name())
        .and_then(|s| s.to_str())
        == Some("pipewire-pulse")
        && field(client, "client.api") == Some("pipewire-pulse")
        && field(client, "config.name") == Some("pipewire-pulse.conf")
        && field(client, "pulse.server.type") == Some("unix")
        && number(node, "application.process.id") == Some(selected.process_id)
        && number(client, "application.process.id") == Some(selected.process_id)
        && reported_binary_matches(node)
        && reported_binary_matches(client)
}

impl Registry {
    pub(super) fn read() -> Result<Self, String> {
        Self::read_with_timeout(Duration::from_secs(2))
    }

    fn read_until(deadline: Instant) -> Result<Self, String> {
        Self::read_with_timeout(remaining(deadline)?)
    }

    fn read_with_timeout(timeout: Duration) -> Result<Self, String> {
        let output = mixrelay_linux_audio_io::bounded_pipewire_output("pw-dump", &[], timeout)
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "pw-dump failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Self::parse(&output.stdout)
    }

    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut objects = BTreeMap::new();
        let mut saw_batch = false;
        for batch in serde_json::Deserializer::from_slice(bytes).into_iter::<Vec<Value>>() {
            saw_batch = true;
            for object in batch.map_err(|e| e.to_string())? {
                let id = number(&object, "id").ok_or("PipeWire object without ID")?;
                if object.get("info") == Some(&Value::Null) {
                    objects.remove(&id);
                } else {
                    merge(objects.entry(id).or_insert(Value::Null), object);
                }
            }
        }
        if !saw_batch {
            return Err("empty PipeWire registry".into());
        }
        Ok(Self(objects))
    }

    fn object(&self, id: u32, kind: &str) -> Option<&Value> {
        self.0.get(&id).filter(|o| field(o, "type") == Some(kind))
    }

    fn client(&self, id: u32) -> Option<(ProcessIdentity, u32)> {
        let props = self
            .object(id, "PipeWire:Interface:Client")?
            .pointer("/info/props")?;
        let pid = number(props, "pipewire.sec.pid")?;
        let uid = number(props, "pipewire.sec.uid")?;
        let _client_serial = serial(props.get("object.serial")?)?;
        Some((process_identity(pid)?, uid))
    }

    fn client_serial_for_node(&self, id: u32) -> Option<String> {
        let node = self
            .object(id, "PipeWire:Interface:Node")?
            .pointer("/info/props")?;
        let client = self
            .object(number(node, "client.id")?, "PipeWire:Interface:Client")?
            .pointer("/info/props")?;
        serial(client.get("object.serial")?)
    }

    pub(super) fn verified_node(
        &self,
        id: u32,
        serial_expected: &str,
        class: &str,
        process: &ProcessIdentity,
    ) -> bool {
        let Some(props) = self
            .object(id, "PipeWire:Interface:Node")
            .and_then(|o| o.pointer("/info/props"))
        else {
            return false;
        };
        if serial(props.get("object.serial").unwrap_or(&Value::Null)).as_deref()
            != Some(serial_expected)
            || field(props, "media.class") != Some(class)
        {
            return false;
        }
        let Some((owner, uid)) = number(props, "client.id").and_then(|id| self.client(id)) else {
            return false;
        };
        // SAFETY: geteuid only reads this process credential; PipeWire's sec.uid
        // is server-provided, unlike application.process.user on the node.
        uid == unsafe { libc::geteuid() } && same_process(&owner, process)
    }

    fn source_owner(
        &self,
        id: u32,
        serial_expected: &str,
        selected: &ProcessIdentity,
    ) -> Option<ProcessIdentity> {
        let node = self
            .object(id, "PipeWire:Interface:Node")?
            .pointer("/info/props")?;
        if serial(node.get("object.serial")?)?.as_str() != serial_expected
            || field(node, "media.class") != Some("Stream/Output/Audio")
        {
            return None;
        }
        let client_id = number(node, "client.id")?;
        let (owner, uid) = self.client(client_id)?;
        if uid != unsafe { libc::geteuid() } {
            return None;
        }
        if same_process(&owner, selected) {
            return Some(owner);
        }
        let client = self
            .object(client_id, "PipeWire:Interface:Client")?
            .pointer("/info/props")?;
        pulse_claim_matches(&owner, selected, node, client).then_some(owner)
    }

    pub(super) fn endpoint(
        &self,
        id: u32,
        native_name: &str,
        driver_owner: &ProcessIdentity,
    ) -> Option<String> {
        let props = self
            .object(id, "PipeWire:Interface:Node")?
            .pointer("/info/props")?;
        if field(props, "node.name") != Some(native_name) {
            return None;
        }
        let serial = serial(props.get("object.serial")?)?;
        self.verified_node(id, &serial, "Audio/Sink", driver_owner)
            .then_some(serial)
    }

    fn metadata_named(&self, name: &str) -> Option<&Value> {
        self.0.values().find(|object| {
            field(object, "type") == Some("PipeWire:Interface:Metadata")
                && object
                    .pointer("/props/metadata.name")
                    .and_then(Value::as_str)
                    == Some(name)
        })
    }

    fn metadata(&self) -> Option<&Value> {
        self.metadata_named(METADATA_NAME)
    }

    // The default metadata is the policy's routing target, not an app-supplied
    // node property. The outer Option rejects an absent or malformed metadata object.
    fn routing_target(&self, source_id: u32) -> Option<ObservedTarget> {
        let metadata = self.metadata_named("default")?;
        let owner_id = number(metadata.get("props")?, "client.id")?;
        let (owner, uid) = self.client(owner_id)?;
        // SAFETY: geteuid reads this process credential without dereferencing a pointer.
        if uid != unsafe { libc::geteuid() }
            || owner
                .executable_path
                .as_deref()
                .and_then(|path| std::path::Path::new(path).file_name())
                .and_then(|name| name.to_str())
                != Some("wireplumber")
        {
            return None;
        }
        routing_target_value(metadata, source_id)
    }

    // An application assertion cannot authorize a lease. A later change can
    // only invalidate the already server-verified owner and pinned route.
    fn source_target_hint(&self, source_id: u32) -> Option<ObservedTarget> {
        let props = self
            .object(source_id, "PipeWire:Interface:Node")?
            .pointer("/info/props")?;
        match props.get("target.object") {
            None | Some(Value::Null) => Some(ObservedTarget::Unset),
            Some(Value::String(value)) => Some(ObservedTarget::Target(value.clone())),
            _ => None,
        }
    }

    // Compare server-assigned node serials, not transient IDs or stereo Link IDs.
    // Capture taps are Stream/Input/Audio and are not playback sink routes.
    fn other_sink_serials(&self, lease: &Lease) -> Option<BTreeSet<String>> {
        let mut routes = BTreeSet::new();
        for object in self.0.values() {
            if field(object, "type") != Some("PipeWire:Interface:Link") {
                continue;
            }
            let info = object.get("info")?;
            let props = info.get("props");
            let output = number(info, "output-node-id")
                .or_else(|| props.and_then(|p| number(p, "link.output.node")));
            if output != Some(lease.source_id) {
                continue;
            }
            let input = number(info, "input-node-id")
                .or_else(|| props.and_then(|p| number(p, "link.input.node")))?;
            if input == lease.endpoint_id {
                continue;
            }
            let node = self.object(input, "PipeWire:Interface:Node")?;
            let node_props = node.pointer("/info/props")?;
            match field(node_props, "media.class")? {
                "Audio/Sink" => {
                    routes.insert(serial(node_props.get("object.serial")?)?);
                }
                "Stream/Input/Audio" => {}
                _ => return None,
            }
        }
        Some(routes)
    }

    pub(super) fn hook_ready(&self) -> bool {
        let Some(metadata) = self.metadata() else {
            return false;
        };
        let Some(owner_id) = metadata
            .pointer("/props/client.id")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
        else {
            return false;
        };
        let Some((owner, uid)) = self.client(owner_id) else {
            return false;
        };
        if uid != unsafe { libc::geteuid() }
            || owner
                .executable_path
                .as_deref()
                .and_then(|p| std::path::Path::new(p).file_name())
                .and_then(|p| p.to_str())
                != Some("wireplumber")
        {
            return false;
        }
        let Some(props) = self
            .object(owner_id, "PipeWire:Interface:Client")
            .and_then(|o| o.pointer("/info/props"))
        else {
            return false;
        };
        field(props, "application.version").is_some_and(|version| version.starts_with("0.5."))
    }

    fn metadata_value(&self, subject: u32, key: &str) -> Option<&str> {
        self.metadata()?
            .get("metadata")?
            .as_array()?
            .iter()
            .find(|item| {
                number(item, "subject") == Some(subject) && field(item, "key") == Some(key)
            })
            .and_then(|item| field(item, "value"))
    }

    pub(super) fn ack(&self, lease: &Lease) -> bool {
        self.hook_ready()
            && self.metadata_value(
                lease.endpoint_id,
                &format!("ack.{}", lease.key.source_serial),
            ) == Some(lease.value().as_str())
    }

    pub(super) fn links(&self, lease: &Lease) -> Vec<(u32, bool)> {
        self.0
            .iter()
            .filter_map(|(id, object)| {
                if field(object, "type") != Some("PipeWire:Interface:Link") {
                    return None;
                }
                let info = object.get("info")?;
                let props = info.get("props");
                let output = number(info, "output-node-id")
                    .or_else(|| props.and_then(|p| number(p, "link.output.node")));
                let input = number(info, "input-node-id")
                    .or_else(|| props.and_then(|p| number(p, "link.input.node")));
                if output != Some(lease.source_id) || input != Some(lease.endpoint_id) {
                    return None;
                }
                let managed = props
                    .and_then(|p| number(p, "client.id"))
                    .and_then(|id| self.client(id))
                    .is_some_and(|(owner, _)| {
                        owner
                            .executable_path
                            .as_deref()
                            .and_then(|p| std::path::Path::new(p).file_name())
                            .and_then(|p| p.to_str())
                            == Some("wireplumber")
                    });
                Some((*id, managed))
            })
            .collect()
    }
}

fn remaining(deadline: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| "PipeWire source-mute command deadline expired".to_owned())
}

fn run_until(program: &str, args: &[String], deadline: Instant) -> Result<(), String> {
    let borrowed = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output =
        mixrelay_linux_audio_io::bounded_pipewire_output(program, &borrowed, remaining(deadline)?)
            .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn metadata_set(lease: &Lease, deadline: Instant) -> Result<(), String> {
    run_until(
        "pw-metadata",
        &[
            "-n".into(),
            METADATA_NAME.into(),
            lease.endpoint_id.to_string(),
            format!("lease.{}", lease.key.source_serial),
            lease.value(),
        ],
        deadline,
    )
}

fn metadata_clear_args(lease: &Lease) -> [String; 5] {
    [
        "-d".into(),
        "-n".into(),
        METADATA_NAME.into(),
        lease.endpoint_id.to_string(),
        format!("lease.{}", lease.key.source_serial),
    ]
}

pub(super) fn metadata_clear(lease: &Lease) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(250);
    run_until("pw-metadata", &metadata_clear_args(lease), deadline)?;
    loop {
        let registry = Registry::read_until(deadline)?;
        let key = format!("lease.{}", lease.key.source_serial);
        if registry.metadata_value(lease.endpoint_id, &key).is_none() && !registry.ack(lease) {
            return Ok(());
        }
        std::thread::sleep(remaining(deadline)?.min(Duration::from_millis(20)));
    }
}

pub(super) fn new_nonce() -> Result<String, String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|e| e.to_string())?;
    let mut nonce = String::with_capacity(32);
    for byte in bytes {
        nonce.push(char::from(HEX[usize::from(byte >> 4)]));
        nonce.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(nonce)
}

/// Installs only after `WirePlumber`'s matching ACK, then removes only existing
/// `WirePlumber`-owned links from the selected stream to this owned endpoint.
pub(super) fn install(lease: &mut Lease) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(500);
    let before = Registry::read_until(deadline)?;
    lease.other_sink_serials = before
        .other_sink_serials(lease)
        .ok_or("could not pin source playback sink routes")?;
    lease.routing_target = before
        .routing_target(lease.source_id)
        .ok_or("could not pin WirePlumber playback routing target")?
        .into_option();
    lease.source_target_hint = before
        .source_target_hint(lease.source_id)
        .ok_or("could not pin source playback routing hint")?
        .into_option();
    let result = (|| {
        metadata_set(lease, deadline)?;
        loop {
            let registry = Registry::read_until(deadline)?;
            if registry.ack(lease) {
                if !registry
                    .source_owner(
                        lease.source_id,
                        &lease.key.source_serial,
                        &lease.source_process,
                    )
                    .is_some_and(|owner| same_process(&owner, &lease.source_owner))
                    || !registry.verified_node(
                        lease.endpoint_id,
                        &lease.key.endpoint_serial,
                        "Audio/Sink",
                        &lease.endpoint_process,
                    )
                    || registry
                        .client_serial_for_node(lease.endpoint_id)
                        .as_deref()
                        != Some(lease.endpoint_client_serial.as_str())
                    || registry.client_serial_for_node(lease.source_id).as_deref()
                        != Some(lease.source_client_serial.as_str())
                {
                    break;
                }
                let links = registry.links(lease);
                if links.iter().any(|(_, managed)| !managed) {
                    break;
                }
                for (id, _) in links {
                    run_until("pw-cli", &["destroy".into(), id.to_string()], deadline)?;
                }
                let after = Registry::read_until(deadline)?;
                if lease_still_valid(&after, lease) {
                    return Ok(());
                }
                break;
            }
            std::thread::sleep(remaining(deadline)?.min(Duration::from_millis(20)));
        }
        Err("WirePlumber did not acknowledge an isolated owned playback route".into())
    })();
    if result.is_err()
        && let Err(error) = metadata_clear(lease)
    {
        tracing::warn!(%error, "could not compensate failed source-mute installation");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_sink_authority_is_pinned_to_canonical_name_serial_and_process() {
        let driver_owner = process_identity(std::process::id()).unwrap();
        // SAFETY: geteuid reads this test process's credential without pointer access.
        let uid = unsafe { libc::geteuid() };
        let name = "io.mixrelay.endpoint.fixture.render";
        let objects = serde_json::json!([
            {"id":9,"type":"PipeWire:Interface:Client","info":{"props":{
                "object.serial":91,"pipewire.sec.pid":driver_owner.process_id,"pipewire.sec.uid":uid}}},
            {"id":12,"type":"PipeWire:Interface:Node","info":{"props":{
                "object.serial":121,"client.id":9,"node.name":name,"media.class":"Audio/Sink",
                "application.id":"io.mixrelay.engine","application.process.id":999_999}}}
        ]);
        let registry = Registry::parse(&serde_json::to_vec(&objects).unwrap()).unwrap();
        assert_eq!(
            registry.endpoint(12, name, &driver_owner).as_deref(),
            Some("121")
        );
        assert_eq!(registry.client_serial_for_node(12).as_deref(), Some("91"));
        assert!(
            registry
                .endpoint(12, "another-device.render", &driver_owner)
                .is_none()
        );
        let mut stale_owner = driver_owner.clone();
        stale_owner.start_time += 1;
        assert!(registry.endpoint(12, name, &stale_owner).is_none());
        let mut other_process = driver_owner;
        other_process.process_id = 999_999;
        assert!(registry.endpoint(12, name, &other_process).is_none());
        let mut recycled = objects;
        recycled[1]["info"]["props"]["object.serial"] = serde_json::json!(122);
        let registry = Registry::parse(&serde_json::to_vec(&recycled).unwrap()).unwrap();
        assert!(!registry.verified_node(12, "121", "Audio/Sink", &stale_owner));
    }

    #[test]
    fn spent_lease_deadline_rejects_command_before_spawn() {
        let expired = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .expect("the monotonic test clock has advanced at least one millisecond");
        assert!(remaining(expired).is_err());
        assert!(run_until("pw-cli", &["destroy".into(), "1".into()], expired).is_err());
    }

    #[test]
    fn server_client_identity_not_node_claim_controls_lease_target() {
        let pid = std::process::id();
        let uid = unsafe { libc::geteuid() };
        let bytes = serde_json::to_vec(&serde_json::json!([
            {"id": 9, "type": "PipeWire:Interface:Client", "info": {"props": {
                "object.serial": 91, "pipewire.sec.pid": pid, "pipewire.sec.uid": uid
            }}},
            {"id": 12, "type": "PipeWire:Interface:Node", "info": {"props": {
                "object.serial": 121, "client.id": 9,
                "media.class": "Stream/Output/Audio", "application.process.id": 999_999
            }}}
        ]))
        .unwrap();
        let registry = Registry::parse(&bytes).unwrap();
        let process = process_identity(pid).unwrap();
        assert_eq!(registry.client_serial_for_node(12).as_deref(), Some("91"));
        assert!(registry.verified_node(12, "121", "Stream/Output/Audio", &process));
        assert!(!registry.verified_node(12, "122", "Stream/Output/Audio", &process));
        assert!(!registry.verified_node(12, "121", "Audio/Sink", &process));
        let mut recycled = process.clone();
        recycled.start_time += 1;
        assert!(!registry.verified_node(12, "121", "Stream/Output/Audio", &recycled));
    }

    #[test]
    fn pulse_broker_requires_actual_broker_executable_and_matching_app_claim() {
        let selected = ProcessIdentity {
            process_id: 163_453,
            parent_id: 1,
            start_time: 91,
            executable_path: Some("/usr/lib/firefox-esr/firefox-esr".into()),
        };
        let mut broker = ProcessIdentity {
            process_id: 654,
            parent_id: 1,
            start_time: 12,
            executable_path: Some("/usr/bin/pipewire-pulse".into()),
        };
        let node = serde_json::json!({
            "application.process.id": 163_453,
            "application.process.binary": "firefox-esr"
        });
        let mut client = serde_json::json!({
            "application.process.id": 163_453,
            "application.process.binary": "firefox-esr",
            "client.api": "pipewire-pulse", "config.name": "pipewire-pulse.conf",
            "pulse.server.type": "unix"
        });
        assert!(pulse_claim_matches(&broker, &selected, &node, &client));
        broker.executable_path = Some("/usr/bin/unrelated".into());
        assert!(!pulse_claim_matches(&broker, &selected, &node, &client));
        broker.executable_path = Some("/usr/bin/pipewire-pulse".into());
        client["application.process.id"] = serde_json::json!(4000);
        assert!(!pulse_claim_matches(&broker, &selected, &node, &client));
    }

    #[test]
    fn pending_apply_keeps_old_lease_until_owner_death_or_reroute() {
        let process = process_identity(std::process::id()).unwrap();
        let key = LeaseKey {
            endpoint_serial: "55".into(),
            source_serial: "121".into(),
        };
        let lease = Lease {
            key: key.clone(),
            endpoint_id: 55,
            source_id: 12,
            endpoint_process: process.clone(),
            source_process: process.clone(),
            source_owner: process.clone(),
            endpoint_client_serial: "1".into(),
            source_client_serial: "2".into(),
            capture_healthy: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            capture_fresh: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            nonce: "old-active-plan".into(),
            references: 1,
            other_sink_serials: BTreeSet::new(),
            routing_target: None,
            source_target_hint: None,
        };
        let mut state = LeaseState::default();
        state.leases.insert(key.clone(), lease);
        let no_links = Registry(BTreeMap::new());
        assert!(
            pending_transition_stale_keys(&state, |old| {
                same_process(&old.source_process, &process) && no_links.links(old).is_empty()
            })
            .is_empty()
        );
        assert!(state.leases.contains_key(&key));

        let current = &state.leases[&key];
        assert!(current.capture_ready());
        current
            .capture_fresh
            .store(false, std::sync::atomic::Ordering::Release);
        assert_eq!(
            pending_transition_stale_keys(&state, Lease::capture_ready),
            vec![key.clone()]
        );
        current
            .capture_fresh
            .store(true, std::sync::atomic::Ordering::Release);
        current
            .capture_healthy
            .store(false, std::sync::atomic::Ordering::Release);
        assert!(!current.capture_ready());
        current
            .capture_healthy
            .store(true, std::sync::atomic::Ordering::Release);

        let mut recycled = process.clone();
        recycled.start_time += 1;
        assert_eq!(
            pending_transition_stale_keys(&state, |old| {
                same_process(&old.source_process, &recycled) && no_links.links(old).is_empty()
            }),
            vec![key.clone()]
        );
        let reroute = Registry::parse(
            br#"[{"id":40,"type":"PipeWire:Interface:Link","info":{"output-node-id":12,"input-node-id":55}}]"#,
        )
        .unwrap();
        assert_eq!(
            pending_transition_stale_keys(&state, |old| {
                same_process(&old.source_process, &process) && reroute.links(old).is_empty()
            }),
            vec![key]
        );
    }

    fn route_fixture() -> (Lease, Vec<Value>) {
        let process = process_identity(std::process::id()).unwrap();
        let lease = Lease {
            key: LeaseKey {
                endpoint_serial: "55".into(),
                source_serial: "121".into(),
            },
            endpoint_id: 55,
            source_id: 12,
            endpoint_process: process.clone(),
            source_process: process.clone(),
            source_owner: process,
            endpoint_client_serial: "1".into(),
            source_client_serial: "2".into(),
            capture_healthy: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            capture_fresh: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            nonce: "nonce".into(),
            references: 1,
            other_sink_serials: BTreeSet::new(),
            routing_target: Some("owned.sink".into()),
            source_target_hint: Some("owned.sink".into()),
        };
        let objects = vec![
            serde_json::json!({"id":70,"type":"PipeWire:Interface:Node","info":{"props":{
                "object.serial":700,"media.class":"Audio/Sink"}}}),
            serde_json::json!({"id":90,"type":"PipeWire:Interface:Node","info":{"props":{
                "object.serial":900,"media.class":"Stream/Input/Audio"}}}),
            serde_json::json!({"id":20,"type":"PipeWire:Interface:Link","info":{
                "output-node-id":12,"input-node-id":55}}),
            serde_json::json!({"id":21,"type":"PipeWire:Interface:Link","info":{
                "output-node-id":12,"input-node-id":70}}),
            serde_json::json!({"id":22,"type":"PipeWire:Interface:Link","info":{
                "output-node-id":12,"input-node-id":70}}),
            serde_json::json!({"id":23,"type":"PipeWire:Interface:Link","info":{
                "output-node-id":12,"input-node-id":90}}),
            serde_json::json!({"id":12,"type":"PipeWire:Interface:Node","info":{"props":{
                "object.serial":121,"media.class":"Stream/Output/Audio",
                "target.object":"owned.sink"}}}),
        ];
        (lease, objects)
    }

    #[test]
    fn metadata_clear_deletes_only_the_exact_owned_lease_key() {
        let (lease, _) = route_fixture();
        let expected: [String; 5] = [
            "-d".into(),
            "-n".into(),
            METADATA_NAME.into(),
            "55".into(),
            "lease.121".into(),
        ];
        assert_eq!(metadata_clear_args(&lease), expected);
    }

    #[test]
    fn installed_route_baseline_ignores_taps_and_stereo_relinks_but_detects_new_sinks() {
        let (mut lease, mut objects) = route_fixture();
        let snapshot =
            |objects: &[Value]| Registry::parse(&serde_json::to_vec(objects).unwrap()).unwrap();
        let baseline = snapshot(&objects).other_sink_serials(&lease).unwrap();
        assert_eq!(baseline, BTreeSet::from(["700".to_owned()]));
        lease.other_sink_serials = baseline;
        assert_eq!(
            snapshot(&objects).source_target_hint(lease.source_id),
            Some(ObservedTarget::Target("owned.sink".into()))
        );
        assert!(route_matches(
            &lease,
            &snapshot(&objects).other_sink_serials(&lease).unwrap(),
            Some("owned.sink")
        ));

        // A stereo Link ID can be recreated without changing its sink owner.
        objects[4]["id"] = serde_json::json!(24);
        assert!(route_matches(
            &lease,
            &snapshot(&objects).other_sink_serials(&lease).unwrap(),
            Some("owned.sink")
        ));
        // Removing an existing other sink does not interfere with that user's route.
        objects.retain(|object| !matches!(object["id"].as_u64(), Some(21 | 24)));
        assert!(route_matches(
            &lease,
            &snapshot(&objects).other_sink_serials(&lease).unwrap(),
            Some("owned.sink")
        ));
        assert!(!route_matches(
            &lease,
            &snapshot(&objects).other_sink_serials(&lease).unwrap(),
            Some("other.sink")
        ));
        objects.push(
            serde_json::json!({"id":80,"type":"PipeWire:Interface:Node","info":{"props":{
            "object.serial":800,"media.class":"Audio/Sink"}}}),
        );
        objects.push(
            serde_json::json!({"id":25,"type":"PipeWire:Interface:Link","info":{
            "output-node-id":12,"input-node-id":80}}),
        );
        assert!(!route_matches(
            &lease,
            &snapshot(&objects).other_sink_serials(&lease).unwrap(),
            Some("owned.sink")
        ));
        // Reusing a transient node ID with a new server serial is a new route.
        objects.retain(|object| !matches!(object["id"].as_u64(), Some(25 | 80)));
        objects[0]["info"]["props"]["object.serial"] = serde_json::json!(701);
        objects.push(
            serde_json::json!({"id":26,"type":"PipeWire:Interface:Link","info":{
            "output-node-id":12,"input-node-id":70}}),
        );
        assert!(!route_matches(
            &lease,
            &snapshot(&objects).other_sink_serials(&lease).unwrap(),
            Some("owned.sink")
        ));
    }

    #[test]
    fn changed_source_hint_revokes_even_when_other_sink_serials_and_policy_target_match() {
        let (mut lease, mut objects) = route_fixture();
        let snapshot =
            |objects: &[Value]| Registry::parse(&serde_json::to_vec(objects).unwrap()).unwrap();
        lease.other_sink_serials = snapshot(&objects).other_sink_serials(&lease).unwrap();
        let source = objects
            .iter()
            .position(|object| object["id"] == 12)
            .unwrap();
        objects[source]["info"]["props"]["target.object"] = serde_json::json!("old-other.sink");
        assert!(route_matches(
            &lease,
            &snapshot(&objects).other_sink_serials(&lease).unwrap(),
            Some("owned.sink")
        ));
        assert_ne!(
            snapshot(&objects).source_target_hint(lease.source_id),
            Some(ObservedTarget::Target("owned.sink".into()))
        );
        objects[source]["info"]["props"]["target.object"] = Value::Null;
        assert_eq!(
            snapshot(&objects).source_target_hint(lease.source_id),
            Some(ObservedTarget::Unset)
        );
        objects[source]["info"]["props"]["target.object"] = serde_json::json!(42);
        assert_eq!(snapshot(&objects).source_target_hint(lease.source_id), None);
    }

    #[test]
    fn routing_target_reads_policy_metadata_and_rejects_unverified_owner() {
        let metadata = serde_json::json!({"metadata":[
            {"subject":12,"key":"target.object","value":"owned.sink"},
            {"subject":13,"key":"target.object","value":"other.sink"}
        ]});
        assert_eq!(
            routing_target_value(&metadata, 12),
            Some(ObservedTarget::Target("owned.sink".into()))
        );
        assert_eq!(
            routing_target_value(&metadata, 14),
            Some(ObservedTarget::Unset)
        );
        let mut malformed = metadata.clone();
        malformed["metadata"][0]
            .as_object_mut()
            .unwrap()
            .remove("value");
        assert_eq!(routing_target_value(&malformed, 12), None);
        let mut changed = metadata.clone();
        changed["metadata"][0]["value"] = serde_json::json!("other.sink");
        assert_eq!(
            routing_target_value(&changed, 12),
            Some(ObservedTarget::Target("other.sink".into()))
        );

        let pid = std::process::id();
        let uid = unsafe { libc::geteuid() };
        let registry = Registry::parse(
            &serde_json::to_vec(&serde_json::json!([
                {"id":9,"type":"PipeWire:Interface:Client","info":{"props":{
                    "object.serial":91,"pipewire.sec.pid":pid,"pipewire.sec.uid":uid}}},
                {"id":30,"type":"PipeWire:Interface:Metadata","props":{
                    "metadata.name":"default","client.id":9},"metadata":metadata["metadata"]}
            ]))
            .unwrap(),
        )
        .unwrap();
        // The test process is not WirePlumber even if it claims a default metadata name.
        assert_eq!(registry.routing_target(12), None);
    }

    #[test]
    fn metadata_and_link_readback_require_exact_subject_and_owner() {
        let bytes = serde_json::to_vec(&serde_json::json!([
            {"id": 30, "type": "PipeWire:Interface:Metadata", "props": {
                "metadata.name": "mixrelay-source-mute", "client.id": 9
            }, "metadata": [
                {"subject": 12, "key": "ack.121", "value": "55:121:1:2:nonce"},
                {"subject": 13, "key": "ack.121", "value": "wrong"}
            ]},
            {"id": 40, "type": "PipeWire:Interface:Link", "info": {
                "output-node-id": 12, "input-node-id": 55,
                "props": {"client.id": 9}
            }}
        ]))
        .unwrap();
        let registry = Registry::parse(&bytes).unwrap();
        assert_eq!(
            registry.metadata_value(12, "ack.121"),
            Some("55:121:1:2:nonce")
        );
        assert_eq!(registry.metadata_value(13, "ack.121"), Some("wrong"));
        assert_eq!(registry.metadata_value(12, "ack.122"), None);
        let process = process_identity(std::process::id()).unwrap();
        let lease = Lease {
            key: LeaseKey {
                endpoint_serial: "55".into(),
                source_serial: "121".into(),
            },
            endpoint_id: 55,
            source_id: 12,
            endpoint_process: process.clone(),
            source_process: process.clone(),
            source_owner: process,
            endpoint_client_serial: "1".into(),
            source_client_serial: "2".into(),
            capture_healthy: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            capture_fresh: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            nonce: "nonce".into(),
            references: 1,
            other_sink_serials: BTreeSet::new(),
            routing_target: None,
            source_target_hint: None,
        };
        assert_eq!(registry.links(&lease), vec![(40, false)]);
    }

    #[test]
    fn appended_registry_removal_does_not_retain_old_link() {
        let bytes = b"[{\"id\":40,\"type\":\"PipeWire:Interface:Link\",\"info\":{\"output-node-id\":12,\"input-node-id\":55}}][{\"id\":40,\"info\":null}]";
        let registry = Registry::parse(bytes).unwrap();
        assert!(registry.0.is_empty());
    }
}
