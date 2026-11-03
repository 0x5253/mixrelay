//! Revision-fenced MIDI command preparation from the local controller snapshot.
use mixrelay_control_protocol::{NodeControlRequest, RecorderControlRequest};
use mixrelay_midi_control::{Action, Binding, Target as MidiTarget};
use serde_json::{Value, json};

/// A prepared action uses the same typed engine request as local controllers.
#[derive(Debug)]
pub enum MidiCommand {
    Node {
        session: String,
        target: MidiTarget,
        request: NodeControlRequest,
    },
    Recorder {
        session: String,
        target: MidiTarget,
        request: RecorderControlRequest,
    },
}

/// Looks up exactly the bound graph and node without falling back to selection.
#[must_use]
pub fn node<'a>(snapshot: &'a Value, target: &MidiTarget) -> Option<&'a Value> {
    snapshot["graphs"]
        .as_array()?
        .iter()
        .find(|graph| graph["id"] == target.graph_id)?["nodes"]
        .as_array()?
        .iter()
        .find(|node| node["nodeId"] == target.node_id)
}

/// Prepares an admitted action only after capabilities and active revisions agree.
#[must_use]
pub fn prepare(snapshot: &Value, binding: &Binding, gain: Option<f32>) -> Option<MidiCommand> {
    let target_node = node(snapshot, &binding.target)?;
    let revision = snapshot["revision"].as_u64()?;
    if snapshot["activeRevision"].as_u64()? < revision {
        return None;
    }
    let session = snapshot["session"].as_str()?.to_owned();
    let target = MidiTarget {
        graph_id: binding.target.graph_id.clone(),
        node_id: binding.target.node_id.clone(),
    };
    let mut request = json!({"expectedRevision": revision});
    let capabilities = &target_node["capabilities"];
    if binding.action == Action::Power {
        if capabilities["power"] != true {
            return None;
        }
        request["enabled"] = json!(!target_node["control"]["enabled"].as_bool()?);
    } else {
        if !running(snapshot, target_node) {
            return None;
        }
        match binding.action {
            Action::Mute if capabilities["mute"] == true => {
                request["muted"] = json!(!target_node["control"]["muted"].as_bool()?);
            }
            Action::Volume if target_node["gainDb"].is_number() => {
                let gain = gain.filter(|gain| gain.is_finite() && (-60.0..=12.0).contains(gain))?;
                request["gainDb"] = json!(gain);
            }
            Action::Record | Action::Pause | Action::Stop if capabilities["recorder"] == true => {
                let recording = &target_node["recorder"];
                let state = recording["state"].as_str()?;
                let action = match binding.action {
                    Action::Record if state != "recording" => "record",
                    Action::Pause if state == "recording" => "pause",
                    Action::Stop if matches!(state, "recording" | "paused") => "stop",
                    _ => return None,
                };
                request["request"] = json!({"action": action});
                request["expectedRecording"] = json!({"state":state, "path":recording["path"]});
                return Some(MidiCommand::Recorder {
                    session,
                    target,
                    request: serde_json::from_value(request).ok()?,
                });
            }
            _ => return None,
        }
    }
    Some(MidiCommand::Node {
        session,
        target,
        request: serde_json::from_value(request).ok()?,
    })
}

fn running(snapshot: &Value, target: &Value) -> bool {
    snapshot["enginePower"]["enabled"] == true
        && snapshot["enginePower"]["transitioning"] == false
        && target["control"]["enabled"] == true
}

/// Whether the bound gain is actionable at the submitted revision.
#[must_use]
pub fn volume_ready(snapshot: &Value, target: &MidiTarget) -> bool {
    node(snapshot, target).is_some_and(|node| running(snapshot, node) && node["gainDb"].is_number())
        && matches!((snapshot["revision"].as_u64(), snapshot["activeRevision"].as_u64()), (Some(revision), Some(active)) if active >= revision)
}

#[cfg(test)]
mod tests;
