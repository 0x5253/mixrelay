//! One `pw-dump` snapshot and the ownership questions source muting asks of it.
//! Authority comes from server-provided client credentials checked against the
//! kernel, never from properties an application sets on its own node.
use super::lease::Lease;
use mixrelay_linux_audio_io::process_identity::{ProcessIdentity, process_identity, same_process};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const METADATA_NAME: &str = "mixrelay-source-mute";

/// Kernel facts that server-provided client credentials are checked against.
#[derive(Clone, Copy)]
pub(super) struct Host {
    /// Effective user ID every trusted client must share.
    pub(super) euid: u32,
    /// Kernel identity of a live process, including its birth time and executable.
    pub(super) identify: fn(u32) -> Option<ProcessIdentity>,
}

impl Host {
    pub(super) fn current() -> Self {
        Self {
            // SAFETY: geteuid only reads this process credential.
            euid: unsafe { libc::geteuid() },
            identify: process_identity,
        }
    }
}

/// A target that `WirePlumber` policy or a stream reports; `Unset` is an explicit
/// absence, distinct from a malformed value.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ObservedTarget {
    Unset,
    Target(String),
}

impl ObservedTarget {
    pub(super) fn as_deref(&self) -> Option<&str> {
        match self {
            Self::Unset => None,
            Self::Target(value) => Some(value),
        }
    }

    pub(super) fn into_option(self) -> Option<String> {
        match self {
            Self::Unset => None,
            Self::Target(value) => Some(value),
        }
    }
}

pub(super) struct Registry {
    objects: BTreeMap<u32, Value>,
    host: Host,
}

impl Registry {
    /// Merges `pw-dump` batches in order; a later `"info": null` removes the object.
    pub(super) fn parse(bytes: &[u8], host: Host) -> Result<Self, String> {
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
        Ok(Self { objects, host })
    }

    pub(super) fn object_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.objects.keys().copied()
    }

    fn object(&self, id: u32, kind: &str) -> Option<&Value> {
        self.objects
            .get(&id)
            .filter(|o| field(o, "type") == Some(kind))
    }

    fn node_props(&self, id: u32) -> Option<&Value> {
        self.object(id, "PipeWire:Interface:Node")?
            .pointer("/info/props")
    }

    fn client_props(&self, id: u32) -> Option<&Value> {
        self.object(id, "PipeWire:Interface:Client")?
            .pointer("/info/props")
    }

    /// Kernel identity and server-reported user of a client.
    fn client(&self, id: u32) -> Option<(ProcessIdentity, u32)> {
        let props = self.client_props(id)?;
        let pid = number(props, "pipewire.sec.pid")?;
        let uid = number(props, "pipewire.sec.uid")?;
        let _client_serial = serial(props.get("object.serial")?)?;
        Some(((self.host.identify)(pid)?, uid))
    }

    /// A client of this user whose kernel executable is named `executable`.
    fn client_runs(&self, id: u32, executable: &str) -> Option<ProcessIdentity> {
        let (owner, uid) = self.client(id)?;
        (uid == self.host.euid && file_name(owner.executable_path.as_deref()) == Some(executable))
            .then_some(owner)
    }

    pub(super) fn client_serial_for_node(&self, id: u32) -> Option<String> {
        let node = self.node_props(id)?;
        let client = self.client_props(number(node, "client.id")?)?;
        serial(client.get("object.serial")?)
    }

    /// The node exists with this serial and class, and its client is `process` running
    /// as this user. Server-assigned `pipewire.sec.uid` is used, not the node's claim.
    pub(super) fn verified_node(
        &self,
        id: u32,
        serial_expected: &str,
        class: &str,
        process: &ProcessIdentity,
    ) -> bool {
        let Some(props) = self.node_props(id) else {
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
        uid == self.host.euid && same_process(&owner, process)
    }

    /// The process owning a playback stream: the selected process itself, or the
    /// `pipewire-pulse` broker when its client claims exactly that process.
    pub(super) fn source_owner(
        &self,
        id: u32,
        serial_expected: &str,
        selected: &ProcessIdentity,
    ) -> Option<ProcessIdentity> {
        let node = self.node_props(id)?;
        if serial(node.get("object.serial")?)?.as_str() != serial_expected
            || field(node, "media.class") != Some("Stream/Output/Audio")
        {
            return None;
        }
        let client_id = number(node, "client.id")?;
        let (owner, uid) = self.client(client_id)?;
        if uid != self.host.euid {
            return None;
        }
        if same_process(&owner, selected) {
            return Some(owner);
        }
        let client = self.client_props(client_id)?;
        pulse_claim_matches(&owner, selected, node, client).then_some(owner)
    }

    /// The serial of the Driver's sink with this exact native name.
    pub(super) fn endpoint(
        &self,
        id: u32,
        native_name: &str,
        driver_owner: &ProcessIdentity,
    ) -> Option<String> {
        let props = self.node_props(id)?;
        if field(props, "node.name") != Some(native_name) {
            return None;
        }
        let serial = serial(props.get("object.serial")?)?;
        self.verified_node(id, &serial, "Audio/Sink", driver_owner)
            .then_some(serial)
    }

    fn metadata_named(&self, name: &str) -> Option<&Value> {
        self.objects.values().find(|object| {
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
    pub(super) fn routing_target(&self, source_id: u32) -> Option<ObservedTarget> {
        let metadata = self.metadata_named("default")?;
        let owner_id = number(metadata.get("props")?, "client.id")?;
        self.client_runs(owner_id, "wireplumber")?;
        routing_target_value(metadata, source_id)
    }

    // An application assertion cannot authorize a lease. A later change can
    // only invalidate the already server-verified owner and pinned route.
    pub(super) fn source_target_hint(&self, source_id: u32) -> Option<ObservedTarget> {
        match self.node_props(source_id)?.get("target.object") {
            None | Some(Value::Null) => Some(ObservedTarget::Unset),
            Some(Value::String(value)) => Some(ObservedTarget::Target(value.clone())),
            _ => None,
        }
    }

    // Compare server-assigned node serials, not transient IDs or stereo Link IDs.
    // Capture taps are Stream/Input/Audio and are not playback sink routes.
    pub(super) fn other_sink_serials(&self, lease: &Lease) -> Option<BTreeSet<String>> {
        let mut routes = BTreeSet::new();
        for object in self.objects.values() {
            if field(object, "type") != Some("PipeWire:Interface:Link") {
                continue;
            }
            let (output, input) = link_ends(object)?;
            if output != Some(lease.source_id) {
                continue;
            }
            let input = input?;
            if input == lease.endpoint_id {
                continue;
            }
            let node_props = self.node_props(input)?;
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

    /// The source-mute metadata exists and belongs to this user's `WirePlumber` 0.5.
    pub(super) fn hook_ready(&self) -> bool {
        let Some(owner_id) = self
            .metadata()
            .and_then(|metadata| metadata.pointer("/props/client.id"))
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
        else {
            return false;
        };
        self.client_runs(owner_id, "wireplumber").is_some()
            && self
                .client_props(owner_id)
                .and_then(|props| field(props, "application.version"))
                .is_some_and(|version| version.starts_with("0.5."))
    }

    pub(super) fn metadata_value(&self, subject: u32, key: &str) -> Option<&str> {
        self.metadata()?
            .get("metadata")?
            .as_array()?
            .iter()
            .find(|item| {
                number(item, "subject") == Some(subject) && field(item, "key") == Some(key)
            })
            .and_then(|item| field(item, "value"))
    }

    /// `WirePlumber` echoed this exact lease value back as its acknowledgement.
    pub(super) fn ack(&self, lease: &Lease) -> bool {
        self.hook_ready()
            && self.metadata_value(
                lease.endpoint_id,
                &format!("ack.{}", lease.key.source_serial),
            ) == Some(lease.value().as_str())
    }

    /// Links from the lease's source to its endpoint, each with whether
    /// `WirePlumber` created it.
    pub(super) fn links(&self, lease: &Lease) -> Vec<(u32, bool)> {
        self.objects
            .iter()
            .filter_map(|(id, object)| {
                if field(object, "type") != Some("PipeWire:Interface:Link") {
                    return None;
                }
                let (output, input) = link_ends(object)?;
                if output != Some(lease.source_id) || input != Some(lease.endpoint_id) {
                    return None;
                }
                let managed = object
                    .pointer("/info/props")
                    .and_then(|p| number(p, "client.id"))
                    .and_then(|id| self.client(id))
                    .is_some_and(|(owner, _)| {
                        file_name(owner.executable_path.as_deref()) == Some("wireplumber")
                    });
                Some((*id, managed))
            })
            .collect()
    }
}

/// Output and input node IDs of a link, read from `info` or its properties.
fn link_ends(object: &Value) -> Option<(Option<u32>, Option<u32>)> {
    let info = object.get("info")?;
    let props = info.get("props");
    let output = number(info, "output-node-id")
        .or_else(|| props.and_then(|p| number(p, "link.output.node")));
    let input =
        number(info, "input-node-id").or_else(|| props.and_then(|p| number(p, "link.input.node")));
    Some((output, input))
}

fn file_name(path: Option<&str>) -> Option<&str> {
    std::path::Path::new(path?).file_name()?.to_str()
}

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
    let Some(binary) = file_name(Some(executable)) else {
        return false;
    };
    let reported_binary_matches = |props: &Value| {
        field(props, "application.process.binary")
            .is_some_and(|reported| reported == binary || reported == executable)
    };
    file_name(owner.executable_path.as_deref()) == Some("pipewire-pulse")
        && field(client, "client.api") == Some("pipewire-pulse")
        && field(client, "config.name") == Some("pipewire-pulse.conf")
        && field(client, "pulse.server.type") == Some("unix")
        && number(node, "application.process.id") == Some(selected.process_id)
        && number(client, "application.process.id") == Some(selected.process_id)
        && reported_binary_matches(node)
        && reported_binary_matches(client)
}

#[cfg(test)]
mod tests;
