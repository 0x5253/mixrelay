//! MIDI and Stream Deck share snapshot fences and the same capture-aware dispatch.
use mixrelay_desktop_application::midi_commands::node;
use mixrelay_midi_control::Binding;
use serde_json::Value;

use super::{Command, LocalControl, Target};

impl LocalControl {
    pub(crate) fn midi_liveness(&self) -> std::sync::Arc<tokio::sync::Notify> {
        std::sync::Arc::clone(&self.client.operation_liveness)
    }

    pub(crate) async fn midi_snapshot(&self) -> Option<Value> {
        self.snapshot()
            .await
            .ok()
            .filter(super::super::ControlProxyResponse::is_success)
            .map(|response| response.body)
    }

    pub(crate) async fn midi_action(
        &self,
        binding: &Binding,
        gain: Option<(f32, Option<f32>)>,
        admission: &crate::midi::admission::Admission<'_>,
    ) -> bool {
        let Some(Some(snapshot)) = admission.while_current(self.midi_snapshot()).await else {
            return false;
        };
        if let Some((_, observed)) = gain {
            let current = node(&snapshot, &binding.target)
                .and_then(|node| serde_json::from_value::<f32>(node["gainDb"].clone()).ok());
            if !matches!((observed, current), (Some(observed), Some(current)) if (observed - current).abs() <= 0.01)
            {
                return false;
            }
        }
        let Some(command) = prepare(&snapshot, binding, gain.map(|(gain, _)| gain)) else {
            return false;
        };
        // Mapping changes and the final write share admission. Once admitted, an
        // accepted mutation finishes once; stale/uncertain writes are never replayed.
        let Some(guard) = admission.enter().await else {
            return false;
        };
        let response = self.execute(command).await;
        drop(guard);
        match response {
            Ok(response) if response.is_success() => {
                if response.body["state"] == "pending" {
                    self.wait_for_midi_revision(&response, admission).await
                } else {
                    true
                }
            }
            Ok(_) => false,
            Err(error) => {
                tracing::warn!(%error, "MIDI control unavailable");
                false
            }
        }
    }

    async fn wait_for_midi_revision(
        &self,
        accepted: &super::super::ControlProxyResponse,
        admission: &crate::midi::admission::Admission<'_>,
    ) -> bool {
        let Some(revision) = accepted.body["revision"].as_u64() else {
            return false;
        };
        loop {
            let Some(Ok(observed)) = admission.while_current(self.snapshot()).await else {
                return false;
            };
            if !observed.is_success() || observed.engine != accepted.engine {
                return false;
            }
            if observed.body["activeRevision"]
                .as_u64()
                .is_some_and(|active| active >= revision)
            {
                return admission.current();
            }
            if admission
                .while_current(tokio::time::sleep(std::time::Duration::from_millis(5)))
                .await
                .is_none()
            {
                return false;
            }
        }
    }
}

fn prepare(snapshot: &Value, binding: &Binding, gain: Option<f32>) -> Option<Command> {
    use mixrelay_desktop_application::midi_commands::{self, MidiCommand};
    Some(match midi_commands::prepare(snapshot, binding, gain)? {
        MidiCommand::Node {
            session,
            target,
            request,
        } => Command::Node {
            session,
            target: Target {
                graph_id: target.graph_id,
                node_id: target.node_id,
            },
            request,
        },
        MidiCommand::Recorder {
            session,
            target,
            request,
        } => Command::Recorder {
            session,
            target: Target {
                graph_id: target.graph_id,
                node_id: target.node_id,
            },
            request,
        },
    })
}

#[cfg(test)]
mod tests;
