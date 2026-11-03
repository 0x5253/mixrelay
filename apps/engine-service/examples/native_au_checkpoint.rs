#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a lab tool, not shipped: setup failures abort with their message"
)]
//! Opt-in macOS Apple AU qualification. No HTTP server, HAL, hardware, or editor window.
//! Run: `cargo run -p mixrelay-engine-service --example native_au_checkpoint -- --run`
//! The parent supplies a newly created private configuration directory to one child.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This opt-in qualification requires macOS.");
}

#[cfg(target_os = "macos")]
fn main() {
    if let Err(error) = qualification::dispatch() {
        eprintln!("native AU qualification failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
mod qualification {
    use std::{
        path::{Path, PathBuf},
        process::Command,
        sync::Arc,
    };

    use mixrelay_audio_graph::{
        BuiltInNodeResolver, CompileContext, CompileLimits, GraphCompiler, GraphDocument,
        NodeConfig, NodeId, NodeSpec, ProviderNodeConfig, RenderFormat,
    };
    use mixrelay_audio_io::{
        HostAudioBackendError, VirtualEndpointBackend, VirtualEndpointCapabilities,
        VirtualEndpointSpec,
    };
    use mixrelay_control_protocol::{
        DriverState, GraphApplyRequest, GraphCatalogAction, GraphCatalogRequest,
        GraphProcessorParameterChange, ProcessorParameterUpdate,
    };
    use mixrelay_control_server::ControlBackend;
    use mixrelay_engine_core::{
        FixedRuntimeStatus, NoopGraphResourceBackend, default_peak_meter_bank,
    };
    use mixrelay_engine_service::{EngineGraphController, EngineGraphControllerConfig};
    use mixrelay_processor_host::{
        PLUGIN_PROBE_ARGUMENT, PLUGIN_SCAN_ARGUMENT, PLUGIN_WORKER_ARGUMENT, ProcessorCatalog,
        RackFormat, RackLocator,
    };
    use mixrelay_processor_state::{StateRef, StateStore};
    use mixrelay_render::{BlockProcessor, ProcessorExchange, RenderPlan};

    const FIXTURE: &str = "aufx:lmtr:appl";
    const FORMAT: RenderFormat = RenderFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };

    pub(super) fn dispatch() -> Result<(), String> {
        let mut args = std::env::args().skip(1);
        match args.next().as_deref() {
            Some("--run") => run_parent(),
            Some("--qualify") => qualify(),
            Some("--enumerate-au") => mixrelay_processor_host::run_plugin_scan(RackFormat::Au, &[]),
            Some(PLUGIN_SCAN_ARGUMENT) => scan(args.next().as_deref()),
            Some(PLUGIN_PROBE_ARGUMENT) => {
                let locator = RackLocator::decode(&args.next().ok_or("missing locator")?)?;
                require_fixture(&locator)?;
                mixrelay_processor_host::run_plugin_probe(locator)
            }
            Some(PLUGIN_WORKER_ARGUMENT) => {
                let values = args.collect::<Vec<_>>();
                let locator = RackLocator::decode(values.first().ok_or("missing locator")?)?;
                require_fixture(&locator)?;
                mixrelay_processor_host::run_plugin_worker(&values)
            }
            _ => Err("explicit --run opt-in is required".into()),
        }
    }

    fn require_fixture(locator: &RackLocator) -> Result<(), String> {
        if locator.unique_id == FIXTURE && matches!(locator.format, RackFormat::Au) {
            Ok(())
        } else {
            Err("qualification refuses to load any non-fixture plug-in".into())
        }
    }

    fn scan(format: Option<&str>) -> Result<(), String> {
        if format != Some("au") {
            println!("{{\"locators\":[],\"pathFailures\":[]}}");
            return Ok(());
        }
        let output = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
            .arg("--enumerate-au")
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err("AU enumeration failed".into());
        }
        let report: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
        let locators = report["locators"]
            .as_array()
            .ok_or("AU enumeration did not return scanner locators")?;
        let selected: Vec<_> = locators
            .iter()
            .filter(|value| value["uniqueId"] == FIXTURE && value["vendor"] == "Apple")
            .collect();
        if selected.len() != 1 {
            return Err("exactly one in-box Apple limiter is required".into());
        }
        println!(
            "{}",
            serde_json::json!({"locators": selected, "pathFailures": []})
        );
        Ok(())
    }

    fn run_parent() -> Result<(), String> {
        let directory =
            mixrelay_test_support::FixtureDir::new("native-au").map_err(|e| e.to_string())?;
        let status = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
            .arg("--qualify")
            .env("MIXRELAY_CONFIG_DIR", directory.path())
            .status()
            .map_err(|e| e.to_string())?;
        eprintln!(
            "Owned qualification catalog (removed on exit): {}",
            directory.display()
        );
        if status.success() {
            Ok(())
        } else {
            Err(format!("qualification child exited {status}"))
        }
    }

    struct NoDevices(VirtualEndpointCapabilities);
    impl VirtualEndpointBackend for NoDevices {
        fn capabilities(&self) -> &VirtualEndpointCapabilities {
            &self.0
        }
        fn prepare_graph_attachments(
            &self,
            endpoints: &[VirtualEndpointSpec],
        ) -> Result<(), HostAudioBackendError> {
            assert!(
                endpoints.is_empty(),
                "qualification must never create an audio endpoint"
            );
            Ok(())
        }
    }

    struct FixtureEngine {
        controller: Arc<EngineGraphController>,
        current: Box<BlockProcessor>,
        exchange: Arc<ProcessorExchange>,
    }
    struct NativeState {
        reference: StateRef,
        bytes: Vec<u8>,
    }
    struct AppleFixture {
        catalog: ProcessorCatalog,
        type_id: String,
        display_name: String,
        parameter: u32,
    }

    impl FixtureEngine {
        fn new(catalog: ProcessorCatalog) -> Result<Self, String> {
            let document = GraphDocument::new(1, vec![], vec![]);
            let plan = GraphCompiler::new(BuiltInNodeResolver)
                .compile(
                    &document,
                    CompileContext {
                        format: FORMAT,
                        limits: CompileLimits::default(),
                    },
                )
                .map_err(|e| format!("{e:?}"))?;
            let current = Box::new(
                BlockProcessor::new(Arc::new(
                    RenderPlan::prepare(&plan).map_err(|e| format!("{e:?}"))?,
                ))
                .unwrap(),
            );
            let exchange = Arc::new(ProcessorExchange::new(1));
            let controller = Arc::new(
                EngineGraphController::new_with_initial_document(
                    exchange.clone(),
                    EngineGraphControllerConfig {
                        clock: std::sync::Arc::new(mixrelay_time::StdClock::new()),
                        format: FORMAT,
                        meters: default_peak_meter_bank(),
                        initial_bindings: vec![],
                        endpoint_backend: Arc::new(NoDevices(VirtualEndpointCapabilities {
                            max_render_channels: 2,
                            max_capture_channels: 2,
                            supported_sample_rates: vec![48_000],
                            supports_duplex: false,
                            supports_dynamic_creation: false,
                        })),
                        required_endpoint_node_id: None,
                        processor_catalog: catalog,
                        graph_resources: Arc::new(NoopGraphResourceBackend),
                        runtime_status: Arc::new(FixedRuntimeStatus::new(DriverState::Ready)),
                    },
                    document,
                )
                .with_graph_catalog(),
            );
            let mut engine = Self {
                controller,
                current,
                exchange,
            };
            engine.activate();
            Ok(engine)
        }
        fn activate(&mut self) {
            self.exchange.activate_pending(&mut self.current);
            self.exchange.reclaim_retired();
        }
        fn mutate(&mut self, action: GraphCatalogAction) {
            let before = self.controller.graph_catalog().unwrap();
            self.controller
                .mutate_graph_catalog(GraphCatalogRequest {
                    expected_revision: before.document.revision,
                    expected_catalog_revision: before.catalog_revision,
                    action,
                })
                .unwrap();
            self.activate();
        }
        fn state(&self, store: &StateStore) -> NativeState {
            let graph = self.controller.graph();
            let provider = graph
                .nodes
                .iter()
                .find_map(|node| match &node.config {
                    NodeConfig::Provider(provider) => Some(provider),
                    _ => None,
                })
                .expect("fixture provider");
            assert!(
                provider.parameters.get("parameterValues").is_none(),
                "native checkpoint must replace stale overlays"
            );
            let reference: StateRef =
                serde_json::from_value(provider.parameters["stateRef"].clone())
                    .expect("validated native state reference");
            let bytes = store.read(&reference).expect("verified native state blob");
            assert_eq!(
                u64::try_from(bytes.len()).expect("state length fits u64"),
                reference.byte_length(),
                "resolved state length must match its content reference"
            );
            NativeState { reference, bytes }
        }
    }

    fn discover_apple_fixture() -> AppleFixture {
        let catalog = ProcessorCatalog::discover();
        let found = catalog.protocol_catalog();
        assert_eq!(found.processors.len(), 1);
        assert!(
            found.diagnostics.is_empty(),
            "fixture scan failed: {:?}",
            found.diagnostics
        );
        let descriptor = found.processors[0].clone();
        assert_eq!(descriptor.vendor, "Apple");
        let parameter = descriptor
            .parameters
            .iter()
            .find(|p| !p.is_read_only)
            .expect("writable limiter parameter")
            .id;
        AppleFixture {
            catalog,
            type_id: descriptor.type_id.as_str().to_owned(),
            display_name: descriptor.display_name,
            parameter,
        }
    }

    fn verify_persisted_blob(
        config: &Path,
        store: &StateStore,
        expected: &NativeState,
    ) -> Result<(), String> {
        let disk: serde_json::Value = serde_json::from_slice(
            &std::fs::read(config.join("graphs.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let disk_reference: StateRef = serde_json::from_value(
            disk["graphs"][0]["document"]["nodes"][0]["config"]["parameters"]["stateRef"].clone(),
        )
        .map_err(|error| format!("invalid catalog state reference: {error}"))?;
        assert_eq!(disk_reference, expected.reference);
        assert_eq!(
            store.read(&disk_reference).map_err(|e| e.to_string())?,
            expected.bytes
        );
        Ok(())
    }

    fn qualify() -> Result<(), String> {
        let config =
            PathBuf::from(std::env::var_os("MIXRELAY_CONFIG_DIR").ok_or("private config missing")?);
        if !config.is_absolute() || config.join("graphs.json").exists() {
            return Err("expected a new private catalog directory".into());
        }
        let store = StateStore::new(config.join("graphs.json").with_extension("states"));
        let AppleFixture {
            catalog,
            type_id,
            display_name,
            parameter,
        } = discover_apple_fixture();
        let mut engine = FixtureEngine::new(catalog.clone())?;
        engine
            .controller
            .apply_graph_request(
                GraphDocument::new(
                    2,
                    vec![NodeSpec {
                        control: mixrelay_audio_graph::NodeControl::default(),
                        id: NodeId::from("effect"),
                        label: Some("Owned Apple AU fixture".into()),
                        config: NodeConfig::Provider(ProviderNodeConfig {
                            ports: None,
                            type_id: type_id.as_str().into(),
                            parameters: serde_json::json!({}),
                        }),
                    }],
                    vec![],
                )
                .into(),
            )
            .unwrap();
        engine.activate();
        edit_and_checkpoint_processor(&engine, parameter);
        let state_a = engine.state(&store);
        assert!(!state_a.bytes.is_empty());
        exercise_switch_and_undo(&mut engine, &type_id, parameter, &store, &state_a);
        engine.controller.start_control_maintenance()?.finish()?;
        verify_persisted_blob(&config, &store, &state_a)?;
        drop(engine);
        let restarted = FixtureEngine::new(catalog)?;
        restarted
            .controller
            .checkpoint_processor_state(None)
            .unwrap();
        let restarted_state = restarted.state(&store);
        assert_eq!(restarted_state.reference, state_a.reference);
        assert_eq!(
            restarted_state.bytes, state_a.bytes,
            "durable catalog restart must restore native state"
        );
        println!(
            "PASS Apple {} parameter={} native_state_bytes={} native_state_sha256={} explicit checkpoint, graph switch, explicit Undo, monitor shutdown, fresh controller+worker restart (state bytes only; no editor or PCM)",
            display_name,
            parameter,
            state_a.reference.byte_length(),
            state_a.reference.sha256()
        );
        Ok(())
    }

    fn edit_and_checkpoint_processor(engine: &FixtureEngine, parameter: u32) {
        let expected_revision = engine.controller.graph().revision;
        engine
            .controller
            .set_processor_parameter(
                "effect",
                parameter,
                ProcessorParameterUpdate {
                    expected_revision,
                    value: 0.37,
                },
            )
            .unwrap();
        engine
            .controller
            .checkpoint_processor_state(Some("effect"))
            .unwrap();
    }

    fn exercise_switch_and_undo(
        engine: &mut FixtureEngine,
        type_id: &str,
        parameter: u32,
        store: &StateStore,
        state_a: &NativeState,
    ) {
        engine.mutate(GraphCatalogAction::Create {
            name: "Empty isolation fixture".into(),
        });
        engine.mutate(GraphCatalogAction::Select {
            id: "graph-1".into(),
        });
        engine.controller.checkpoint_processor_state(None).unwrap();
        let selected = engine.state(store);
        assert_eq!(selected.reference, state_a.reference);
        assert_eq!(
            selected.bytes, state_a.bytes,
            "selected worker must restore native state exactly"
        );
        engine
            .controller
            .set_processor_parameter(
                "effect",
                parameter,
                ProcessorParameterUpdate {
                    expected_revision: engine.controller.graph().revision,
                    value: 0.91,
                },
            )
            .unwrap();
        engine.controller.checkpoint_processor_state(None).unwrap();
        let edited = engine.state(store);
        assert_ne!(edited.reference, state_a.reference);
        assert_ne!(
            edited.bytes, state_a.bytes,
            "native parameter edit must change stored state bytes"
        );
        let mut proposed = engine.controller.graph();
        proposed.revision += 1;
        engine
            .controller
            .apply_graph_request(GraphApplyRequest {
                document: proposed,
                processor_parameter_changes: vec![GraphProcessorParameterChange {
                    node_id: "effect".into(),
                    type_id: type_id.to_owned(),
                    parameter_id: parameter,
                    value: 0.37,
                }],
                expected_graph_id: None,
            })
            .unwrap();
        engine.activate();
        engine.controller.checkpoint_processor_state(None).unwrap();
        let undo = engine.state(store);
        assert_eq!(undo.reference, state_a.reference);
        assert_eq!(
            undo.bytes, state_a.bytes,
            "explicit Undo delta must restore native parameter"
        );
    }
}
