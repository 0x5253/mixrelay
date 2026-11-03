//! Graph device selection and deterministic aggregate membership, without IO.
use mixrelay_audio_graph::{DeviceSelector, GraphDocument, NodeConfig};
use mixrelay_engine_core::resource_requirements::DeviceRequirements;
use mixrelay_macos_transport::endpoint_identity::{endpoint_uid, parse_endpoint_id};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::CString,
};
/// Whether the selector names one of this product's own virtual devices.
pub(super) fn is_virtual_selection(selector: &DeviceSelector) -> bool {
    let DeviceSelector::PersistentUid { uid } = selector else {
        return false;
    };
    uid.strip_prefix("io.mixrelay.audio.endpoint.")
        .is_some_and(|hex| {
            hex.len() == 32
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                && parse_endpoint_id(&format!(
                    "{}-{}-{}-{}-{}",
                    &hex[..8],
                    &hex[8..12],
                    &hex[12..16],
                    &hex[16..20],
                    &hex[20..]
                ))
                .is_some()
        })
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Spec {
    pub(super) selector: DeviceSelector,
    pub(super) input: bool,
    pub(super) channels: u16,
}
impl Spec {
    pub(super) fn native_uid(&self) -> Result<CString, String> {
        let uid = match (&self.selector, self.input) {
            (DeviceSelector::PersistentUid { uid }, _) if !uid.is_empty() => uid.as_str(),
            (DeviceSelector::DefaultInput, true) | (DeviceSelector::DefaultOutput, false) => "",
            _ => return Err("wrong device selector direction or empty UID".into()),
        };
        CString::new(uid).map_err(|error| error.to_string())
    }
}
pub(super) fn specs(document: &GraphDocument) -> BTreeMap<String, Spec> {
    let requirements = DeviceRequirements::from_document(document);
    let mut selected: BTreeMap<_, _> = requirements
        .inputs
        .into_iter()
        .map(|(id, config)| {
            (
                id,
                Spec {
                    selector: config.selector,
                    input: true,
                    channels: config.output_channels,
                },
            )
        })
        .chain(requirements.outputs.into_iter().map(|(id, config)| {
            (
                id,
                Spec {
                    selector: config.selector,
                    input: false,
                    channels: config.input_channels,
                },
            )
        }))
        .collect();
    for node in &document.nodes {
        let NodeConfig::VirtualEndpointTransport(endpoint) = &node.config else {
            continue;
        };
        let Some(id) = parse_endpoint_id(endpoint.id.as_str()) else {
            continue;
        };
        for (input, stream, suffix) in [
            (true, endpoint.capture, "/capture"),
            (false, endpoint.render, "/render"),
        ] {
            // Owned virtual inputs use the HAL's timestamped shared client queues.
            // Only output clients enter CoreAudio; input-size cache state is not shared
            // between the engine's graph quantum and external application readers.
            if !input && let Some(stream) = stream {
                selected.insert(
                    format!("{}{suffix}", node.id.as_str()),
                    Spec {
                        selector: DeviceSelector::PersistentUid {
                            uid: endpoint_uid(id),
                        },
                        input,
                        channels: stream.channels,
                    },
                );
            }
        }
    }
    selected
}

/// Fixed selections only. A default selector continues following the OS default
/// rather than accidentally pinning yesterday's device inside an aggregate.
/// Prefer an output as primary; ordering is stable and shared with channel maps.
pub(super) fn aggregate_members(specs: &BTreeMap<String, Spec>) -> Vec<String> {
    let mut uids = BTreeSet::new();
    let mut primary = None;
    for spec in specs.values() {
        if let DeviceSelector::PersistentUid { uid } = &spec.selector {
            // MixRelay virtual devices stay independent clients outside the aggregate.
            if is_virtual_selection(&spec.selector) {
                continue;
            }
            uids.insert(uid.clone());
            if !spec.input && primary.is_none() {
                primary = Some(uid.clone());
            }
        }
    }
    let primary = primary.or_else(|| uids.first().cloned());
    let mut members = Vec::new();
    if let Some(primary) = primary {
        uids.remove(&primary);
        members.push(primary);
    }
    members.extend(uids);
    members
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixrelay_audio_graph::composition::{
        DeviceRegistry, ResolvedNodeIdentities, compose, lower_document,
    };
    use mixrelay_audio_graph::{
        AudioDeviceInputConfig, AudioDeviceOutputConfig, CompileContext, CompileLimits,
        GraphCompiler, NodeConfig, NodeSpec, RenderFormat,
    };
    use mixrelay_engine_core::session::AdapterRuntimeStatus;
    use std::sync::Arc;

    fn assert_no_native_owners(selected: BTreeMap<String, Spec>, format: RenderFormat) {
        use super::super::{MacDevices, simulated::SimulatedHost};
        let host = SimulatedHost::new();
        let status = Arc::new(AdapterRuntimeStatus::new(
            crate::macos::adapter_profile(),
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        ));
        let resources = MacDevices::with_host(status, 48_000, 128, host.clone());
        let (owners, cache) = resources
            .prepare_device_set(&BTreeMap::new(), selected, format)
            .unwrap();
        assert!(owners.0.is_empty());
        assert!(owners.1.is_empty());
        assert!(cache.is_empty());
        assert_eq!(host.registrations(), 0);
    }

    #[test]
    fn qualification_recorder_uses_shared_virtual_input_without_changing_driver_geometry() {
        let document: GraphDocument = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../crates/audio-graph/src/fixtures/qualification-stereo.json"
        )))
        .unwrap();
        let NodeConfig::VirtualEndpoint(endpoint) = &document.nodes[0].config else {
            panic!("qualification source is a virtual endpoint")
        };
        assert_eq!(document.nodes[0].id.as_str(), "qa-source");
        assert!(endpoint.reference.render);
        assert!(!endpoint.reference.capture);
        let canonical = endpoint.spec.clone();
        assert_eq!(canonical.render.unwrap().channels, 2);
        assert_eq!(canonical.capture.unwrap().channels, 2);
        let registry = DeviceRegistry::new(vec![canonical.clone()]).unwrap();
        let authored =
            lower_document(&document, &registry, &ResolvedNodeIdentities::default()).unwrap();
        let context = CompileContext {
            format: RenderFormat {
                sample_rate: 48_000,
                block_frames: 128,
            },
            limits: CompileLimits::default(),
        };
        let composed = compose(
            &registry,
            &[authored.as_fragment("qualification-stereo")],
            document.revision,
            context,
            &GraphCompiler::default(),
        )
        .unwrap();
        let selected = specs(&composed.document);
        assert!(composed.native_devices.contains_key(&canonical.id));
        assert!(
            selected.is_empty(),
            "a graph recorder must not create a CoreAudio input reader"
        );
        assert!(aggregate_members(&selected).is_empty());
        assert_eq!(registry.get(&canonical.id), Some(&canonical));
        assert_eq!(endpoint.spec, canonical);
        assert_no_native_owners(selected, context.format);
    }

    #[test]
    fn driver_output_clients_keep_native_uid_while_capture_uses_shared_queues() {
        let mut document: GraphDocument = serde_json::from_value(serde_json::json!({
            "schemaVersion": 2, "revision": 1, "nodes": [{
                "id":"driver", "type":"virtual_endpoint",
                "config":{"id":"01010101-0101-0101-0101-010101010101","displayName":"Fixture",
                    "reference":{"render":true,"capture":true},
                    "render":{"channels":7,"channelLayout":"discrete"},
                    "capture":{"channels":3,"channelLayout":"discrete"},
                    "supportedSampleRates":[48000],"defaultSampleRate":48000}
            }], "edges":[]
        }))
        .unwrap();
        if let NodeConfig::VirtualEndpoint(endpoint) = &document.nodes[0].config {
            document.nodes[0].config = NodeConfig::VirtualEndpointTransport(endpoint.spec.clone());
        }
        let selected = specs(&document);
        assert_eq!(selected.len(), 1);
        assert!(!selected.contains_key("driver/capture"));
        assert!(!selected["driver/render"].input);
        assert_eq!(selected["driver/render"].channels, 7);
        let uid = endpoint_uid([1; 16]);
        assert_eq!(
            selected["driver/render"]
                .native_uid()
                .unwrap()
                .to_str()
                .unwrap(),
            uid
        );
        assert!(aggregate_members(&selected).is_empty());
        document.nodes.clear();
        assert!(specs(&document).is_empty());
    }

    #[test]
    fn physical_clock_group_excludes_independent_virtual_clients() {
        let selected = BTreeMap::from([
            (
                "virtual".into(),
                Spec {
                    selector: DeviceSelector::PersistentUid {
                        uid: endpoint_uid([1; 16]),
                    },
                    input: false,
                    channels: 2,
                },
            ),
            (
                "speaker".into(),
                Spec {
                    selector: DeviceSelector::PersistentUid {
                        uid: "speaker".into(),
                    },
                    input: false,
                    channels: 2,
                },
            ),
            (
                "mic".into(),
                Spec {
                    selector: DeviceSelector::PersistentUid { uid: "mic".into() },
                    input: true,
                    channels: 2,
                },
            ),
        ]);
        assert_eq!(aggregate_members(&selected), ["speaker", "mic"]);
    }

    #[test]
    fn shared_requirements_preserve_native_direction_width_and_output_clock_priority() {
        let document = GraphDocument::new(
            1,
            vec![
                NodeSpec {
                    control: mixrelay_audio_graph::NodeControl::default(),
                    id: "mic".into(),
                    label: None,
                    config: NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                        selector: DeviceSelector::PersistentUid {
                            uid: "a-mic".into(),
                        },
                        output_channels: 8,
                        gain_db: -6.0,
                    }),
                },
                NodeSpec {
                    control: mixrelay_audio_graph::NodeControl::default(),
                    id: "speaker".into(),
                    label: None,
                    config: NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                        selector: DeviceSelector::PersistentUid {
                            uid: "z-speaker".into(),
                        },
                        input_channels: 2,
                    }),
                },
            ],
            vec![],
        );
        let selected = specs(&document);
        assert!(selected["mic"].input);
        assert_eq!(selected["mic"].channels, 8);
        assert!(!selected["speaker"].input);
        assert_eq!(selected["speaker"].channels, 2);
        assert_eq!(
            selected["mic"].native_uid().unwrap().to_str().unwrap(),
            "a-mic"
        );
        assert_eq!(aggregate_members(&selected), ["z-speaker", "a-mic"]);
    }
}
