//! Device-targeted negotiation must not silently resume a mismatched old graph.
use super::*;
use crate::document_files::memory::MemoryDocuments;
use mixrelay_audio_io::VirtualEndpointSettings;
use mixrelay_control_protocol::VirtualEndpointFormatRequest;
use mixrelay_processor_state::{preview_bundle, write_bundle};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[path = "endpoint_format_ack_tests.rs"]
mod acknowledgements;

struct NativeState {
    rate: AtomicU32,
    peer_rate: AtomicU32,
    calls: AtomicU32,
    writes: Mutex<Vec<VirtualEndpointId>>,
    resumed: AtomicU32,
    failed: AtomicBool,
    reject_rollback: AtomicBool,
    reject_after_write: AtomicBool,
    change_during_suspend: AtomicBool,
    primary_clock: AtomicBool,
}
struct Runtime {
    state: Arc<NativeState>,
    base: Arc<dyn RuntimeBackend>,
}
impl RuntimeStatusBackend for Runtime {
    fn status(&self, format: RenderFormat) -> AppStatus {
        self.base.status(format)
    }
}

impl ProcessingControlBackend for Runtime {
    fn suspend(&self) -> Result<(), HostAudioBackendError> {
        if self.state.change_during_suspend.load(Ordering::Acquire) {
            self.state.rate.store(44_100, Ordering::Release);
        }
        Ok(())
    }
    fn fail_reconfiguration(&self) {
        self.state.failed.store(true, Ordering::Release);
    }
    fn resume(&self) {
        self.state.resumed.fetch_add(1, Ordering::Relaxed);
    }
}

impl SessionFormatBackend for Runtime {
    fn external_format(&self, current: RenderFormat) -> Option<RenderFormat> {
        Some(RenderFormat {
            sample_rate: if self.state.primary_clock.load(Ordering::Acquire) {
                self.state.rate.load(Ordering::Acquire)
            } else {
                current.sample_rate
            },
            ..current
        })
    }
    fn adopt_external_format(&self, requested: RenderFormat) -> Result<(), HostAudioBackendError> {
        assert_eq!(
            requested.sample_rate,
            self.state.rate.load(Ordering::Acquire)
        );
        Ok(())
    }
}
struct Endpoint {
    state: Arc<NativeState>,
    base: ContractOnlyVirtualEndpointBackend,
}
impl VirtualEndpointBackend for Endpoint {
    fn capabilities(&self) -> &VirtualEndpointCapabilities {
        self.base.capabilities()
    }
    fn sample_rate(&self, id: &VirtualEndpointId) -> Result<u32, HostAudioBackendError> {
        let rate = match id.as_str() {
            DEFAULT_ENDPOINT_NODE_ID => &self.state.rate,
            "peer-device" => &self.state.peer_rate,
            _ => {
                return Err(HostAudioBackendError::BackendFailure {
                    message: "unknown device identity".into(),
                });
            }
        };
        Ok(rate.load(Ordering::Acquire))
    }
    fn set_sample_rate(
        &self,
        id: &VirtualEndpointId,
        rate: u32,
    ) -> Result<(), HostAudioBackendError> {
        self.state.calls.fetch_add(1, Ordering::Relaxed);
        self.state.writes.lock().push(id.clone());
        if rate == 48_000 && self.state.reject_rollback.load(Ordering::Acquire) {
            return Err(HostAudioBackendError::BackendFailure {
                message: "rollback rejected; device remains at new rate".into(),
            });
        }
        match id.as_str() {
            DEFAULT_ENDPOINT_NODE_ID => self.state.rate.store(rate, Ordering::Release),
            "peer-device" => self.state.peer_rate.store(rate, Ordering::Release),
            _ => {
                return Err(HostAudioBackendError::BackendFailure {
                    message: "unknown device identity".into(),
                });
            }
        }
        if rate == 44_100 && self.state.reject_after_write.load(Ordering::Acquire) {
            return Err(HostAudioBackendError::BackendFailure {
                message: "setter failed after accepting the native rate".into(),
            });
        }
        Ok(())
    }
    fn settings(&self) -> Option<VirtualEndpointSettings> {
        let mut settings = VirtualEndpointSettings::configurable(self.capabilities());
        let document = default_duplex_document(1, None);
        let NodeConfig::VirtualEndpoint(endpoint) = &document.nodes[0].config else {
            panic!()
        };
        settings.devices = [DEFAULT_ENDPOINT_NODE_ID, "peer-device"]
            .into_iter()
            .map(|id| {
                let mut spec = endpoint.spec.clone();
                spec.id = VirtualEndpointId::from(id);
                mixrelay_audio_io::VirtualEndpointDeviceSettings {
                    native_device_ids: Vec::new(),
                    observed_sample_rate: self.sample_rate(&spec.id).ok(),
                    spec,
                    supported_sample_rates: vec![44_100, 48_000],
                    can_set_sample_rate: true,
                    native_formats: mixrelay_audio_io::NativeDeviceFormats::default(),
                }
            })
            .collect();
        Some(settings)
    }
    fn prepare_graph_attachments(
        &self,
        specs: &[VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError> {
        self.base.prepare_graph_attachments(specs)
    }
}
struct Reject;
impl GraphResourceBackend for Reject {
    fn reconcile(
        &self,
        _: &GraphDocument,
        _: crate::PlaybackBindings<'_>,
    ) -> Result<(), HostAudioBackendError> {
        Err(HostAudioBackendError::BackendFailure {
            message: "fixture graph preparation failed".into(),
        })
    }
}
fn fixture() -> (EngineGraphController, Arc<NativeState>) {
    let mut controller = endpoint_settings_tests::controller(default_duplex_document(1, None));
    let state = Arc::new(NativeState {
        rate: AtomicU32::new(48_000),
        peer_rate: AtomicU32::new(48_000),
        calls: AtomicU32::new(0),
        writes: Mutex::new(vec![]),
        resumed: AtomicU32::new(0),
        failed: AtomicBool::new(false),
        reject_rollback: AtomicBool::new(false),
        reject_after_write: AtomicBool::new(false),
        change_during_suspend: AtomicBool::new(false),
        primary_clock: AtomicBool::new(false),
    });
    controller.runtime_status = Arc::new(Runtime {
        state: state.clone(),
        base: controller.runtime_status.clone(),
    });
    controller.endpoint_backend = Arc::new(Endpoint {
        state: state.clone(),
        base: ContractOnlyVirtualEndpointBackend::default(),
    });
    (controller, state)
}
fn request() -> VirtualEndpointFormatRequest {
    VirtualEndpointFormatRequest {
        expected_revision: 1,
        endpoint_id: VirtualEndpointId::from(DEFAULT_ENDPOINT_NODE_ID),
        sample_rate: 44_100,
    }
}

#[test]
fn device_change_preserves_processing_clock_and_updates_endpoint_metadata() {
    let (controller, state) = fixture();
    let processing_format = controller.context.read().format;
    let result = controller.change_endpoint_format(&request()).unwrap();
    assert_eq!(result.revision, 2);
    assert_eq!(state.rate.load(Ordering::Acquire), 44_100);
    assert_eq!(controller.context.read().format.sample_rate, 48_000);
    assert_eq!(controller.context.read().format, processing_format);
    let graph = controller.graph();
    let NodeConfig::VirtualEndpoint(endpoint) = &graph.nodes[0].config else {
        panic!()
    };
    assert_eq!(endpoint.default_sample_rate, 44_100);
    assert_eq!(state.resumed.load(Ordering::Acquire), 1);
}

#[test]
fn partial_native_write_failure_restores_only_the_requested_device() {
    let (controller, state) = fixture();
    state.reject_after_write.store(true, Ordering::Release);
    assert!(controller.change_endpoint_format(&request()).is_err());
    assert_eq!(state.rate.load(Ordering::Acquire), 48_000);
    assert_eq!(state.peer_rate.load(Ordering::Acquire), 48_000);
    assert_eq!(state.calls.load(Ordering::Acquire), 2);
    assert_eq!(state.resumed.load(Ordering::Acquire), 1);
    assert_eq!(controller.graph().revision, 1);
}

#[test]
fn supported_native_pair_round_trip_preserves_processing_clock_and_peer_rate() {
    let (controller, state) = fixture();
    let original = controller.context.read().format;
    let changed = controller.change_endpoint_format(&request()).unwrap();
    let _active = crate::controller_test_support::settle(&controller);
    controller
        .change_endpoint_format(&VirtualEndpointFormatRequest {
            expected_revision: changed.revision,
            sample_rate: 48_000,
            ..request()
        })
        .unwrap();
    assert_eq!(state.rate.load(Ordering::Acquire), 48_000);
    assert_eq!(state.peer_rate.load(Ordering::Acquire), 48_000);
    assert_eq!(controller.context.read().format, original);
    assert_eq!(state.resumed.load(Ordering::Acquire), 2);
}

#[test]
fn device_setter_never_receives_rates_outside_the_native_pair() {
    let (controller, state) = fixture();
    for sample_rate in [32_000, 88_200, 96_000, 192_000] {
        assert!(
            controller
                .change_endpoint_format(&VirtualEndpointFormatRequest {
                    sample_rate,
                    ..request()
                })
                .is_err()
        );
    }
    assert_eq!(state.calls.load(Ordering::Acquire), 0);
    assert_eq!(state.resumed.load(Ordering::Acquire), 0);
}

#[test]
fn unsupported_and_stale_requests_never_change_device() {
    let (controller, state) = fixture();
    assert!(
        controller
            .change_endpoint_format(&VirtualEndpointFormatRequest {
                sample_rate: 192_000,
                ..request()
            })
            .is_err()
    );
    assert!(
        controller
            .change_endpoint_format(&VirtualEndpointFormatRequest {
                expected_revision: 0,
                ..request()
            })
            .is_err()
    );
    assert_eq!(state.calls.load(Ordering::Acquire), 0);
}

#[test]
fn one_device_change_does_not_relabel_or_retune_another_device() {
    let (controller, state) = fixture();
    let mut peer = controller.graph.read().nodes[0].clone();
    peer.id = NodeId::from("peer-node");
    let NodeConfig::VirtualEndpoint(spec) = &mut peer.config else {
        panic!()
    };
    spec.id = VirtualEndpointId::from("peer-device");
    controller.graph.write().nodes.push(peer);
    controller.change_endpoint_format(&request()).unwrap();
    let rates: Vec<_> = controller
        .graph()
        .nodes
        .iter()
        .filter_map(|node| match &node.config {
            NodeConfig::VirtualEndpoint(spec) => Some((spec.id.clone(), spec.default_sample_rate)),
            _ => None,
        })
        .collect();
    assert_eq!(
        rates,
        vec![
            (VirtualEndpointId::from(DEFAULT_ENDPOINT_NODE_ID), 44_100),
            (VirtualEndpointId::from("peer-device"), 48_000),
        ]
    );
    assert_eq!(
        *state.writes.lock(),
        vec![VirtualEndpointId::from(DEFAULT_ENDPOINT_NODE_ID)]
    );
    assert_eq!(controller.context.read().format.sample_rate, 48_000);
}

#[test]
fn unknown_device_and_a_daw_change_during_pause_do_not_issue_native_writes() {
    let (controller, state) = fixture();
    let mut unknown = request();
    unknown.endpoint_id = VirtualEndpointId::from("missing-device");
    assert!(controller.change_endpoint_format(&unknown).is_err());
    state.change_during_suspend.store(true, Ordering::Release);
    assert!(controller.change_endpoint_format(&request()).is_err());
    assert_eq!(state.calls.load(Ordering::Acquire), 0);
    assert_eq!(state.rate.load(Ordering::Acquire), 44_100);
    assert_eq!(controller.graph().revision, 1);
    assert_eq!(state.resumed.load(Ordering::Acquire), 1);
}

#[test]
fn failed_preparation_restores_device_and_failed_rollback_never_resumes() {
    for reject_rollback in [false, true] {
        let (mut controller, state) = fixture();
        controller.graph_resources = Arc::new(Reject);
        state
            .reject_rollback
            .store(reject_rollback, Ordering::Release);
        assert!(controller.change_endpoint_format(&request()).is_err());
        assert_eq!(controller.graph().revision, 1);
        assert_eq!(state.failed.load(Ordering::Acquire), reject_rollback);
        assert_eq!(
            state.resumed.load(Ordering::Acquire),
            u32::from(!reject_rollback)
        );
        assert_eq!(
            state.rate.load(Ordering::Acquire),
            if reject_rollback { 44_100 } else { 48_000 }
        );
    }
}
fn workspace_request(
    controller: &EngineGraphController,
    action: mixrelay_control_protocol::WorkspaceAction,
) -> mixrelay_control_protocol::WorkspaceRequest {
    let catalog = controller.read_catalog().unwrap();
    mixrelay_control_protocol::WorkspaceRequest {
        file_digest: None,
        expected_revision: catalog.document.revision,
        expected_catalog_revision: catalog.catalog_revision,
        operation: action,
    }
}

fn saved_workspace_rate(files: &MemoryDocuments, path: &str, rate: u32) {
    let (manifest, references) = preview_bundle(std::io::Cursor::new(
        files.bytes(std::path::Path::new(path)).unwrap(),
    ))
    .unwrap();
    assert!(references.is_empty());
    let mut file: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    for graph in file["catalog"]["graphs"].as_array_mut().unwrap() {
        let mut document: GraphDocument =
            serde_json::from_value(graph["document"].clone()).unwrap();
        for node in &mut document.nodes {
            if let NodeConfig::VirtualEndpoint(endpoint) = &mut node.config {
                endpoint.spec.default_sample_rate = rate;
                endpoint.spec.supported_sample_rates = vec![44_100, 48_000];
            }
        }
        graph["document"] = serde_json::to_value(document).unwrap();
    }
    let manifest = serde_json::to_vec(&file).unwrap();
    let store = crate::test_support::MemoryAssets::default();
    let mut bundle = Vec::new();
    write_bundle(&mut bundle, &manifest, &references, &store).unwrap();
    files.insert(path, bundle);
}

#[test]
fn workspace_open_adopts_primary_clock_but_preserves_independent_processing_clock() {
    use mixrelay_control_protocol::WorkspaceAction;
    for primary in [false, true] {
        let dir = crate::test_support::Documents::new("workspace-rate");
        let (controller, state) = fixture();
        let controller = dir.catalog(controller);
        state.primary_clock.store(primary, Ordering::Release);
        let path = dir.join("session.mix").to_string_lossy().into_owned();
        controller
            .workspace_operation(workspace_request(
                &controller,
                WorkspaceAction::Save { path: path.clone() },
            ))
            .unwrap();
        saved_workspace_rate(&dir.files, &path, 44_100);
        controller
            .workspace_operation(workspace_request(
                &controller,
                WorkspaceAction::Open { path },
            ))
            .unwrap();
        assert_eq!(state.rate.load(Ordering::Acquire), 44_100);
        assert_eq!(
            controller.context.read().format.sample_rate,
            if primary { 44_100 } else { 48_000 }
        );
        assert_eq!(state.resumed.load(Ordering::Acquire), 1);
    }
}

#[test]
fn workspace_failed_native_preparation_restores_rate_and_authored_catalog() {
    use mixrelay_control_protocol::WorkspaceAction;
    let dir = crate::test_support::Documents::new("workspace-rollback");
    let (controller, state) = fixture();
    let mut controller = dir.catalog(controller);
    let before = controller.read_catalog().unwrap();
    let disk = crate::test_support::read_document(&dir.files, &dir.join("graphs.json")).unwrap();
    let path = dir.join("session.mix").to_string_lossy().into_owned();
    controller
        .workspace_operation(workspace_request(
            &controller,
            WorkspaceAction::Save { path: path.clone() },
        ))
        .unwrap();
    saved_workspace_rate(&dir.files, &path, 44_100);
    controller.graph_resources = Arc::new(Reject);
    assert!(
        controller
            .workspace_operation(workspace_request(
                &controller,
                WorkspaceAction::Open { path }
            ))
            .is_err()
    );
    assert_eq!(state.rate.load(Ordering::Acquire), 48_000);
    assert_eq!(before, controller.read_catalog().unwrap());
    assert_eq!(
        disk,
        crate::test_support::read_document(&dir.files, &dir.join("graphs.json")).unwrap()
    );
}

#[test]
fn device_format_is_persisted_in_catalog_without_a_global_session_rate_write() {
    let directory = crate::test_support::Documents::new("device-format-catalog");
    let path = directory.join("graphs.json");
    let (controller, _) = fixture();
    let controller = controller
        .with_test_documents(&directory.files)
        .with_graph_catalog_path(path.clone());
    controller.change_endpoint_format(&request()).unwrap();
    let saved: serde_json::Value = serde_json::from_slice(
        &crate::test_support::read_document(&directory.files, &path).unwrap(),
    )
    .unwrap();
    let document: GraphDocument =
        serde_json::from_value(saved["graphs"][0]["document"].clone()).unwrap();
    let NodeConfig::VirtualEndpoint(spec) = &document.nodes[0].config else {
        panic!()
    };
    assert_eq!(spec.default_sample_rate, 44_100);
    assert!(
        directory
            .files
            .bytes(&directory.join("audio-session.json"))
            .is_none()
    );
    assert_eq!(controller.context.read().format.sample_rate, 48_000);
}
