use std::{collections::BTreeMap, sync::Mutex};

use mixrelay_audio_engine::ExternalBinding;
use mixrelay_audio_graph::{GraphDocument, NodeConfig, NodeId};
use mixrelay_audio_io::{AudioDeviceDescriptor, DeviceSelector, HostAudioBackendError};
use mixrelay_engine::{GraphResourceBackend, PlaybackBindings};

pub(super) const INPUT_UID: &str = "fixture-input";
pub(super) const TARGET_UID: &str = "fixture-target-output";
pub(super) const HEALTHY_UID: &str = "fixture-healthy-output";

/// Only the graph's device metadata is synthetic; native plug-in workers process real PCM.
/// The reconciled composition supplies its opaque binding IDs, without duplicating the
/// production ID encoding or relying on sink enumeration order.
#[derive(Default)]
pub(super) struct FixtureResources {
    outputs: Mutex<BTreeMap<String, NodeId>>,
}

pub(super) enum SinkOrder {
    TargetFirst,
    HealthyFirst,
    HealthyOnly,
}

impl FixtureResources {
    pub(super) fn sink_order(&self, bindings: &[ExternalBinding]) -> Result<SinkOrder, String> {
        let outputs = self.outputs.lock().map_err(|error| error.to_string())?;
        let healthy = outputs.get(HEALTHY_UID).ok_or("healthy output is absent")?;
        let target = outputs.get(TARGET_UID);
        match bindings {
            [first, second] if Some(first.node_id()) == target && second.node_id() == healthy => {
                Ok(SinkOrder::TargetFirst)
            }
            [first, second] if first.node_id() == healthy && Some(second.node_id()) == target => {
                Ok(SinkOrder::HealthyFirst)
            }
            [only] if target.is_none() && only.node_id() == healthy => Ok(SinkOrder::HealthyOnly),
            _ => Err(format!(
                "unexpected independent sink bindings: {bindings:?}"
            )),
        }
    }
}

impl GraphResourceBackend for FixtureResources {
    fn device_inventory(
        &self,
        selectors: &[&DeviceSelector],
    ) -> Result<Vec<AudioDeviceDescriptor>, HostAudioBackendError> {
        for requested in selectors {
            validate_selector(requested)?;
        }
        Ok([INPUT_UID, TARGET_UID, HEALTHY_UID]
            .into_iter()
            .map(|uid| AudioDeviceDescriptor {
                uid: uid.into(),
                display_name: uid.into(),
                input_channels: if uid == INPUT_UID { 2 } else { 0 },
                output_channels: if uid == INPUT_UID { 0 } else { 2 },
                is_default_input: false,
                is_default_output: false,
            })
            .collect())
    }

    fn reconcile(
        &self,
        document: &GraphDocument,
        playback: PlaybackBindings<'_>,
    ) -> Result<(), HostAudioBackendError> {
        if matches!(playback, PlaybackBindings::Forward { .. }) {
            return Err(unsupported(
                "fixture application playback control is unavailable",
            ));
        }
        let mut outputs = BTreeMap::new();
        for node in &document.nodes {
            match &node.config {
                NodeConfig::AudioDeviceInput(config) => {
                    validate_selector(&config.selector)?;
                }
                NodeConfig::AudioDeviceOutput(config) => {
                    let uid = validate_selector(&config.selector)?;
                    if uid == INPUT_UID || outputs.insert(uid.to_owned(), node.id.clone()).is_some()
                    {
                        return Err(unsupported("fixture outputs must have independent owners"));
                    }
                }
                NodeConfig::ApplicationCapture(_) | NodeConfig::VirtualEndpointTransport(_) => {
                    return Err(unsupported("native capture and endpoints are forbidden"));
                }
                _ => {}
            }
        }
        *self
            .outputs
            .lock()
            .map_err(|_| unsupported("fixture output registry poisoned"))? = outputs;
        Ok(())
    }
}

pub(super) fn selector(uid: &str) -> DeviceSelector {
    DeviceSelector::PersistentUid { uid: uid.into() }
}

fn validate_selector(selector: &DeviceSelector) -> Result<&str, HostAudioBackendError> {
    match selector {
        DeviceSelector::PersistentUid { uid }
            if [INPUT_UID, TARGET_UID, HEALTHY_UID].contains(&uid.as_str()) =>
        {
            Ok(uid)
        }
        _ => Err(unsupported(
            "only explicit fixture I/O identities are allowed",
        )),
    }
}

fn unsupported(reason: &str) -> HostAudioBackendError {
    HostAudioBackendError::CapabilityUnavailable {
        capability: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixrelay_audio_graph::{AudioDeviceOutputConfig, NodeControl, NodeSpec};

    #[test]
    fn fixture_inventory_never_resolves_system_defaults_or_unknown_devices() {
        let resources = FixtureResources::default();
        assert!(
            resources
                .device_inventory(&[&DeviceSelector::DefaultInput])
                .is_err()
        );
        assert!(
            resources
                .device_inventory(&[&DeviceSelector::DefaultOutput])
                .is_err()
        );
        assert!(
            resources
                .device_inventory(&[&selector("user-device")])
                .is_err()
        );
        let inventory = resources
            .device_inventory(&[&selector(TARGET_UID)])
            .unwrap();
        assert_eq!(
            inventory
                .iter()
                .filter(|device| device.output_channels == 2)
                .count(),
            2
        );
        assert!(
            inventory
                .iter()
                .all(|device| !device.is_default_input && !device.is_default_output)
        );
    }

    #[test]
    fn reconcile_refuses_to_combine_two_fixture_outputs_into_one_owner() {
        let nodes = ["first", "second"].map(|id| NodeSpec {
            id: id.into(),
            label: None,
            control: NodeControl::default(),
            config: NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                selector: selector(TARGET_UID),
                input_channels: 2,
            }),
        });
        let document = GraphDocument::new(1, nodes.to_vec(), vec![]);
        assert!(
            FixtureResources::default()
                .reconcile(&document, PlaybackBindings::Preserve)
                .is_err()
        );
    }
}
