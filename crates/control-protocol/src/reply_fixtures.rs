//! Representative engine replies, serialized for the control UI decoder tests.
//!
//! A stale fixture is rewritten and the test fails; review the diff and commit it
//! (`pnpm fixtures:replies` reruns every fixture owner).
use mixrelay_audio_io::recording::RecorderStatus;
use std::collections::BTreeMap;

use mixrelay_audio_graph::{
    AUDIO_PORT_ID, AudioDeviceInputConfig, AudioDeviceOutputConfig, AudioEndpoint, ChannelMapping,
    ChannelRoute, DeviceSelector, EdgeId, EdgeSpec, GainConfig, GraphDocument, NodeConfig,
    NodeControl, NodeId, NodeSpec, PortId,
};
use mixrelay_audio_io::recording::{Encoding, RecorderState};
use mixrelay_audio_io::{
    ChannelLayout, NativeDeviceFormats, NativeSampleFormat, NativeStreamFormat,
    NativeStreamFormats, StreamSpec, VirtualEndpointCapabilities, VirtualEndpointDeviceSettings,
    VirtualEndpointId, VirtualEndpointSettings, VirtualEndpointSpec,
    process::{ProcessingState, SampleFaults},
    session::{
        AudioBackendKind, AudioSessionCapabilities, AudioSessionStatus, ClockAuthority,
        SessionFormat, SessionIssue,
    },
};
use mixrelay_test_support::{every_variant, reply_fixture::refresh_reply_fixture};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    AppStatus, ControlStateSnapshot, DriverState, EngineControlSnapshot, EnginePauseReason,
    EnginePowerStatus, ErrorCode, FailedProcessor, GraphCatalogEntry, GraphCatalogResponse,
    GraphControlSnapshot, GraphLayout, GraphNodePosition, GraphNodeSize, GraphPendingReason,
    GraphRuntimeStatus, GraphViewport, NodeControlCapabilities, NodeControlDescriptor,
    NodeControlSnapshot, PROTOCOL_VERSION, ProcessorEditorState, ProcessorEditorStatus,
    ProcessorLatency, ProcessorTiming,
};

/// The fixture's name when it was stale and has just been rewritten.
fn stale(name: &'static str, value: &impl Serialize) -> Option<&'static str> {
    refresh_reply_fixture(name, value).unwrap().then_some(name)
}

const SESSION_FORMAT: SessionFormat = SessionFormat {
    sample_rate: 48_000,
    block_frames: 128,
};

fn native(channels: u16, sample_format: NativeSampleFormat) -> NativeStreamFormat {
    NativeStreamFormat {
        sample_rate: 48_000,
        channels,
        sample_format,
    }
}

fn virtual_endpoint_settings() -> VirtualEndpointSettings {
    let stereo = StreamSpec {
        channels: 2,
        channel_layout: ChannelLayout::Stereo,
    };
    VirtualEndpointSettings {
        devices: vec![VirtualEndpointDeviceSettings {
            spec: VirtualEndpointSpec {
                id: VirtualEndpointId::new("relay-1".to_owned()),
                display_name: "MixRelay 1".to_owned(),
                render: Some(stereo),
                capture: None,
                supported_sample_rates: vec![44_100, 48_000],
                default_sample_rate: 48_000,
            },
            native_device_ids: vec!["MixRelayDevice:relay-1".to_owned()],
            observed_sample_rate: Some(48_000),
            supported_sample_rates: vec![44_100, 48_000],
            can_set_sample_rate: true,
            native_formats: NativeDeviceFormats {
                render: NativeStreamFormats {
                    current: Some(native(2, NativeSampleFormat::Float32)),
                    supported: vec![
                        native(2, NativeSampleFormat::Float32),
                        native(2, NativeSampleFormat::Signed24),
                    ],
                },
                capture: NativeStreamFormats::default(),
            },
        }],
        ..VirtualEndpointSettings::configurable(&VirtualEndpointCapabilities {
            max_render_channels: 2,
            max_capture_channels: 2,
            supported_sample_rates: vec![44_100, 48_000],
            supports_duplex: true,
            supports_dynamic_creation: true,
        })
    }
}

fn session() -> AudioSessionStatus {
    AudioSessionStatus {
        endpoint_preferences_unavailable: false,
        virtual_endpoint_settings: Some(virtual_endpoint_settings()),
        processing_state: ProcessingState::Running,
        discontinuities: 1,
        format: SESSION_FORMAT,
        preferred_format: Some(SESSION_FORMAT),
        clock_authority: ClockAuthority::Device,
        observed_device_sample_rate: Some(48_000),
        issue: None,
        transport_latency_frames: Some(256),
        capabilities: AudioSessionCapabilities {
            backend: AudioBackendKind::CoreAudio,
            supported_sample_rates: vec![44_100, 48_000, 96_000],
            supported_block_frames: vec![64, 128, 256],
            can_reconfigure_live: true,
            hardware_input: true,
            hardware_output: true,
            virtual_endpoint: true,
            application_capture: true,
            sample_rate_conversion: false,
            drift_correction: false,
        },
    }
}

const POWER: EnginePowerStatus = EnginePowerStatus {
    enabled: false,
    transitioning: false,
    unsaved: true,
    pause_reason: Some(EnginePauseReason::RepeatedFailures),
};

fn status() -> AppStatus {
    AppStatus {
        engine_power: Some(POWER),
        processor_timings: vec![ProcessorTiming {
            graph_id: "graph-1".to_owned(),
            node_id: "gain".to_owned(),
            round_trip_ns: 41_000,
        }],
        processor_latencies: vec![ProcessorLatency {
            graph_id: "graph-1".to_owned(),
            node_id: "gain".to_owned(),
            transport_frames: 128,
            intrinsic_frames: None,
        }],
        failed_processors: vec![FailedProcessor {
            graph_id: "graph-1".to_owned(),
            node_id: "plugin".to_owned(),
            instance_id: u64::MAX.to_string(),
        }],
        recorder_error: Some("disk full".to_owned()),
        protocol_version: PROTOCOL_VERSION,
        driver_state: DriverState::Ready,
        sample_rate: Some(48_000),
        buffer_frames: Some(128),
        underruns: 2,
        overruns: 1,
        sample_faults: SampleFaults {
            source_samples: 0,
            input_samples: 0,
            output_samples: 3,
            blocks: 1,
        },
        audio_session: Some(session()),
    }
}

fn node(id: &str, label: Option<&str>, control: NodeControl, config: NodeConfig) -> NodeSpec {
    NodeSpec {
        id: NodeId::new(id),
        label: label.map(str::to_owned),
        control,
        config,
    }
}

fn edge(source: &str, destination: &str, channel_mapping: ChannelMapping) -> EdgeSpec {
    let endpoint = |node: &str| AudioEndpoint {
        node_id: NodeId::new(node),
        port_id: PortId::new(AUDIO_PORT_ID),
    };
    EdgeSpec {
        id: EdgeId::new(format!("{source}-{destination}")),
        source: endpoint(source),
        destination: endpoint(destination),
        channel_mapping,
    }
}

fn graph() -> GraphDocument {
    GraphDocument::new(
        2,
        vec![
            node(
                "mic",
                Some("Microphone"),
                NodeControl::default(),
                NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                    selector: DeviceSelector::DefaultInput,
                    output_channels: 2,
                    gain_db: 0.0,
                }),
            ),
            node(
                "gain",
                None,
                NodeControl {
                    enabled: true,
                    muted: true,
                },
                NodeConfig::Gain(GainConfig {
                    channels: 2,
                    gain_db: -6.0,
                }),
            ),
            node(
                "speakers",
                Some("Speakers"),
                NodeControl::default(),
                NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                    selector: DeviceSelector::PersistentUid {
                        uid: "BuiltInSpeakerDevice".to_owned(),
                    },
                    input_channels: 2,
                }),
            ),
        ],
        vec![
            edge("mic", "gain", ChannelMapping::Auto),
            edge(
                "gain",
                "speakers",
                ChannelMapping::Explicit {
                    routes: vec![
                        ChannelRoute {
                            source_channel: 0,
                            destination_channel: 1,
                        },
                        ChannelRoute {
                            source_channel: 1,
                            destination_channel: 0,
                        },
                    ],
                },
            ),
        ],
    )
}

fn catalog() -> GraphCatalogResponse {
    let entry = |id: &str, name: &str| GraphCatalogEntry {
        id: id.to_owned(),
        name: name.to_owned(),
    };
    GraphCatalogResponse {
        protocol_version: PROTOCOL_VERSION,
        catalog_revision: 3,
        active_id: "graph-1".to_owned(),
        graphs: vec![entry("graph-1", "Studio"), entry("graph-2", "Podcast")],
        document: graph(),
        layout_revision: 4,
        layout: GraphLayout {
            node_labels: BTreeMap::from([("speakers".to_owned(), "Desk speakers".to_owned())]),
            positions: BTreeMap::from([("mic".to_owned(), GraphNodePosition { x: 0.0, y: 40.0 })]),
            sizes: BTreeMap::from([(
                "gain".to_owned(),
                GraphNodeSize {
                    width: 240.0,
                    height: 160.0,
                },
            )]),
            viewport: Some(GraphViewport {
                x: -20.0,
                y: 10.0,
                zoom: 1.25,
            }),
        },
        persistence_error: Some("disk full".to_owned()),
    }
}

fn control(node_id: &str, kind: &str, recorder: Option<RecorderStatus>) -> NodeControlSnapshot {
    NodeControlSnapshot {
        descriptor: NodeControlDescriptor {
            node_id: node_id.to_owned(),
            label: None,
            kind: kind.to_owned(),
            control: NodeControl::default(),
            capabilities: NodeControlCapabilities {
                power: true,
                mute: true,
                recorder: recorder.is_some(),
            },
            gain_db: recorder.is_none().then_some(-6.0),
        },
        recorder,
    }
}

fn telemetry_state() -> ControlStateSnapshot {
    ControlStateSnapshot {
        generation: 7,
        status: status(),
        graph: GraphRuntimeStatus {
            protocol_version: PROTOCOL_VERSION,
            active_revision: 2,
            submitted_revision: 3,
            pending_reason: Some(GraphPendingReason::AwaitingMicrophonePermission),
        },
        controls: Some(EngineControlSnapshot {
            revision: 2,
            active_revision: 2,
            structure_generation: 1,
            engine_power: POWER,
            graphs: vec![GraphControlSnapshot {
                id: "graph-1".to_owned(),
                name: "Studio".to_owned(),
                nodes: vec![
                    control("gain", "gain", None),
                    control(
                        "take",
                        "recorder",
                        Some(RecorderStatus {
                            state: RecorderState::Recording,
                            path: Some("/Users/example/Music/take.wav".to_owned()),
                            sample_rate: Some(48_000),
                            encoding: Some(Encoding::Pcm24),
                            frames: 96_000,
                            error: None,
                        }),
                    ),
                ],
            }],
        }),
        editors: vec![ProcessorEditorStatus {
            protocol_version: PROTOCOL_VERSION,
            node_id: "plugin".to_owned(),
            state: ProcessorEditorState::Open,
            width: Some(640),
            height: Some(480),
            resizable: true,
            failure: None,
        }],
    }
}

/// Every variant of each status enum the UI decodes, by its field path in `status`.
fn status_variants() -> BTreeMap<String, Value> {
    let device = "audioSession.virtualEndpointSettings.devices[0]";
    BTreeMap::from([
        (
            "driverState".to_owned(),
            json!(every_variant!(DriverState {
                NotInstalled,
                Installing,
                RestartRequired,
                InstalledNotLoaded,
                Blocked,
                Unavailable,
                InvalidPermissions,
                VersionMismatch,
                Ready,
            })),
        ),
        (
            "enginePower.pauseReason".to_owned(),
            json!(every_variant!(EnginePauseReason { RepeatedFailures })),
        ),
        (
            "audioSession.processingState".to_owned(),
            json!(every_variant!(ProcessingState {
                Idle,
                Running,
                Stopped
            })),
        ),
        (
            "audioSession.clockAuthority".to_owned(),
            json!(every_variant!(ClockAuthority {
                EngineTimer,
                Device,
                AudioServer,
                Host,
            })),
        ),
        (
            "audioSession.issue".to_owned(),
            json!(every_variant!(SessionIssue {
                DeviceUnavailable,
                ProcessingFailed,
                ApplicationUnavailable,
                ApplicationAmbiguous,
                SampleRateMismatch,
                ClockNotSynchronized,
                InvalidFormat,
                ReconfigurationFailed,
                PreferencesUnavailable,
            })),
        ),
        (
            "audioSession.capabilities.backend".to_owned(),
            json!(every_variant!(AudioBackendKind {
                CoreAudio,
                PipeWire,
                ContractOnly,
            })),
        ),
        (
            format!("{device}.spec.render.channelLayout"),
            json!(every_variant!(ChannelLayout {
                Mono,
                Stereo,
                Discrete
            })),
        ),
        (
            format!("{device}.nativeFormats.render.supported[0].sampleFormat"),
            json!(every_variant!(NativeSampleFormat {
                Float32,
                Signed16,
                Signed24,
                Signed32
            })),
        ),
    ])
}

/// Every variant of each telemetry-state enum the UI decodes, by its field path.
fn telemetry_state_variants() -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "graph.pendingReason".to_owned(),
            json!(every_variant!(GraphPendingReason {
                AwaitingMicrophonePermission
            })),
        ),
        (
            "editors[0].state".to_owned(),
            json!(every_variant!(ProcessorEditorState {
                Unavailable,
                Closed,
                Opening,
                Open,
                Failed,
            })),
        ),
        ("controls".to_owned(), json!([null])),
    ])
}

/// Every refusal code the engine sends, so the UI can check the codes it interprets exist.
fn error_codes() -> Value {
    json!(every_variant!(ErrorCode {
        DriverNotInstalled,
        DriverNotLoaded,
        DriverPermissionInvalid,
        DriverVersionMismatch,
        AudioDeviceDisconnected,
        AudioPermissionPending,
        AudioPermissionDenied,
        BufferUnderrun,
        BufferOverrun,
        AudioSourceUnavailable,
        PairingRequired,
        Unauthorized,
        ControlUnavailable,
        EngineReplaced,
        InvalidOperationId,
        GraphInvalid,
        GraphRevisionStale,
        GraphExchangeBusy,
        GraphBindingUnsupported,
        NodeNotFound,
        NodeDisabled,
        ProcessorUnavailable,
        RecorderFailed,
        AudioSessionUnsupported,
        AudioSessionProcessorStateUnavailable,
        AudioSessionReconfigureFailed,
    }))
}

#[test]
fn engine_reply_fixtures_match_the_serialized_contracts() {
    let stale: Vec<_> = [
        stale("status", &status()),
        stale("status.variants", &status_variants()),
        stale("graph", &graph()),
        stale("catalog", &catalog()),
        stale("telemetry-state", &telemetry_state()),
        stale("telemetry-state.variants", &telemetry_state_variants()),
        stale("error-codes", &error_codes()),
    ]
    .into_iter()
    .flatten()
    .collect();
    assert!(
        stale.is_empty(),
        "reply fixtures {stale:?} were stale and have been rewritten; review and commit them"
    );
}
