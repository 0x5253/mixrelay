//! Control-plane format transactions must not mix old transports with a new DSP plan.
use super::*;
use crate::controller_test_support::settle;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

struct Runtime {
    base: Arc<dyn RuntimeBackend>,
    rate: AtomicU32,
    frames: AtomicU32,
    endpoint_rate: AtomicU32,
    changes: AtomicU32,
    endpoint_changes: AtomicU32,
    adoptions: AtomicU32,
    fail: AtomicBool,
    negotiated: Mutex<Vec<RenderFormat>>,
}
impl RuntimeStatusBackend for Runtime {
    fn status(&self, format: RenderFormat) -> AppStatus {
        let mut status = self.base.status(format);
        let capabilities = &mut status.audio_session.as_mut().unwrap().capabilities;
        capabilities.supported_block_frames = vec![64, 128, 256];
        capabilities.can_reconfigure_live = true;
        status
    }
}

impl ProcessingControlBackend for Runtime {}

impl SessionFormatBackend for Runtime {
    fn external_sample_rate(&self) -> Option<u32> {
        let rate = self.rate.load(Ordering::Acquire);
        (rate != 0).then_some(rate)
    }
    fn external_format(&self, current: RenderFormat) -> Option<RenderFormat> {
        let frames = self.frames.load(Ordering::Acquire);
        self.external_sample_rate().map(|sample_rate| RenderFormat {
            sample_rate,
            block_frames: if frames == 0 {
                current.block_frames
            } else {
                frames
            },
        })
    }
    fn adopt_external_format(&self, _: RenderFormat) -> Result<(), HostAudioBackendError> {
        self.adoptions.fetch_add(1, Ordering::Relaxed);
        if self.fail.load(Ordering::Acquire) {
            return Err(failure());
        }
        Ok(())
    }
    fn reconfigure(&self, requested: RenderFormat) -> Result<(), HostAudioBackendError> {
        self.changes.fetch_add(1, Ordering::Relaxed);
        self.negotiated.lock().push(requested);
        Ok(())
    }
}
fn failure() -> HostAudioBackendError {
    HostAudioBackendError::BackendFailure {
        message: "fixture native failure".into(),
    }
}
fn fixture() -> (EngineGraphController, Arc<Runtime>) {
    let mut controller = endpoint_settings_tests::controller_with_inventory(
        GraphDocument::new(1, vec![], vec![]),
        &default_duplex_document(1, None),
    );
    let runtime = Arc::new(Runtime {
        base: controller.runtime_status.clone(),
        rate: AtomicU32::new(44_100),
        frames: AtomicU32::new(0),
        endpoint_rate: AtomicU32::new(0),
        changes: AtomicU32::new(0),
        endpoint_changes: AtomicU32::new(0),
        adoptions: AtomicU32::new(0),
        fail: AtomicBool::new(false),
        negotiated: Mutex::new(Vec::new()),
    });
    controller.runtime_status = runtime.clone();
    controller.endpoint_backend = Arc::new(ObservedEndpoint {
        runtime: runtime.clone(),
        base: controller.endpoint_backend.clone(),
    });
    (controller, runtime)
}

struct ObservedEndpoint {
    runtime: Arc<Runtime>,
    base: Arc<dyn VirtualEndpointBackend>,
}
impl VirtualEndpointBackend for ObservedEndpoint {
    fn capabilities(&self) -> &VirtualEndpointCapabilities {
        self.base.capabilities()
    }
    fn settings(&self) -> Option<mixrelay_audio_io::VirtualEndpointSettings> {
        self.base.settings()
    }
    fn sample_rate(&self, _: &VirtualEndpointId) -> Result<u32, HostAudioBackendError> {
        let rate = self.runtime.endpoint_rate.load(Ordering::Acquire);
        if rate == 0 {
            return Err(failure());
        }
        Ok(rate)
    }
    fn set_sample_rate(&self, _: &VirtualEndpointId, _: u32) -> Result<(), HostAudioBackendError> {
        self.runtime
            .endpoint_changes
            .fetch_add(1, Ordering::Relaxed);
        Err(failure())
    }
    fn resolve_graph_devices(
        &self,
        desired: &[VirtualEndpointSpec],
    ) -> Result<Vec<VirtualEndpointSpec>, HostAudioBackendError> {
        let mut actual = self.base.resolve_graph_devices(desired)?;
        let rate = self.runtime.endpoint_rate.load(Ordering::Acquire);
        if rate != 0 {
            for spec in &mut actual {
                spec.default_sample_rate = rate;
                if !spec.supported_sample_rates.contains(&rate) {
                    spec.supported_sample_rates.push(rate);
                }
            }
        }
        Ok(actual)
    }
    fn prepare_graph_attachments(
        &self,
        specs: &[VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError> {
        self.base.prepare_graph_attachments(specs)
    }
}
#[test]
fn catalog_selection_preserves_device_rate_independent_of_hardware_clock() {
    use mixrelay_control_protocol::{GraphCatalogAction, GraphCatalogRequest};
    let directory = crate::test_support::Documents::new("catalog-device-clock");
    let (controller, runtime) = fixture();
    runtime.endpoint_rate.store(48_000, Ordering::Release);
    controller
        .apply_graph_request(default_duplex_document(2, None).into())
        .unwrap();
    settle(&controller);
    controller
        .reconcile_observed_device_rate()
        .unwrap()
        .unwrap();
    settle(&controller);
    let controller = directory.catalog(controller);
    let original = controller.graph_catalog().unwrap().active_id;
    for action in [
        GraphCatalogAction::Create {
            name: "Other graph".into(),
        },
        GraphCatalogAction::Select { id: original },
    ] {
        let catalog = controller.graph_catalog().unwrap();
        let before = controller.exchange.revisions();
        controller
            .mutate_graph_catalog(GraphCatalogRequest {
                expected_revision: controller.graph().revision,
                expected_catalog_revision: catalog.catalog_revision,
                action,
            })
            .unwrap();
        assert_eq!(controller.exchange.revisions(), before);
    }
    assert_eq!(controller.context.read().format.sample_rate, 44_100);
    for node in &controller.graph().nodes {
        if let NodeConfig::VirtualEndpoint(endpoint) = &node.config {
            assert_eq!(endpoint.default_sample_rate, 48_000);
        }
    }
    assert_eq!(runtime.changes.load(Ordering::Acquire), 0);
}

#[test]
fn hardware_clock_adoption_preserves_independent_virtual_device_rate() {
    let (controller, runtime) = fixture();
    runtime.endpoint_rate.store(48_000, Ordering::Release);
    controller
        .apply_graph_request(default_duplex_document(2, None).into())
        .unwrap();
    settle(&controller);
    controller
        .reconcile_observed_device_rate()
        .unwrap()
        .unwrap();
    settle(&controller);
    assert_eq!(controller.context.read().format.sample_rate, 44_100);
    let document = controller.graph();
    let NodeConfig::VirtualEndpoint(endpoint) = &document.nodes[0].config else {
        panic!()
    };
    assert_eq!(endpoint.default_sample_rate, 48_000);
    assert_eq!(runtime.changes.load(Ordering::Acquire), 0);
    assert_eq!(runtime.endpoint_changes.load(Ordering::Acquire), 0);
    assert!(runtime.negotiated.lock().is_empty());
    assert_eq!(runtime.endpoint_rate.load(Ordering::Acquire), 48_000);
    let settings = endpoint_settings_tests::settings(&controller);
    assert_eq!(settings.devices[0].spec.default_sample_rate, 48_000);
}

#[test]
fn independent_endpoint_change_updates_graph_without_retuning_processing_clock() {
    let (controller, runtime) = fixture();
    runtime.rate.store(48_000, Ordering::Release);
    runtime.endpoint_rate.store(48_000, Ordering::Release);
    controller
        .apply_graph_request(default_duplex_document(2, None).into())
        .unwrap();
    settle(&controller);
    runtime.endpoint_rate.store(44_100, Ordering::Release);
    let result = controller
        .reconcile_observed_device_rate()
        .unwrap()
        .unwrap();
    assert_eq!(result.revision, 3);
    settle(&controller);
    assert_eq!(controller.context.read().format.sample_rate, 48_000);
    let document = controller.graph();
    let NodeConfig::VirtualEndpoint(endpoint) = &document.nodes[0].config else {
        panic!()
    };
    assert_eq!(endpoint.default_sample_rate, 44_100);
    assert_eq!(runtime.changes.load(Ordering::Acquire), 0);
    assert_eq!(runtime.adoptions.load(Ordering::Acquire), 0);
    assert!(
        controller
            .reconcile_observed_device_rate()
            .unwrap()
            .is_none()
    );
}

#[test]
fn external_change_rebuilds_once_and_defers_pending_and_concurrent_mutations() {
    let (controller, runtime) = fixture();
    {
        let _lock = controller.mutation.lock();
        assert!(
            controller
                .reconcile_observed_device_rate()
                .unwrap()
                .is_none()
        );
    }
    let result = controller
        .reconcile_observed_device_rate()
        .unwrap()
        .unwrap();
    assert_eq!(result.revision, 2);
    assert_eq!(controller.context.read().format.sample_rate, 44_100);
    runtime.rate.store(48_000, Ordering::Release);
    assert!(
        controller
            .reconcile_observed_device_rate()
            .unwrap()
            .is_none()
    );
    assert_eq!(runtime.adoptions.load(Ordering::Acquire), 1);
    settle(&controller);
    assert_eq!(
        controller
            .reconcile_observed_device_rate()
            .unwrap()
            .unwrap()
            .revision,
        3
    );
    settle(&controller);
    assert!(
        controller
            .reconcile_observed_device_rate()
            .unwrap()
            .is_none()
    );
    assert_eq!(runtime.changes.load(Ordering::Acquire), 0);
}

struct Resources {
    committed: Arc<AtomicU32>,
    cancelled: Arc<AtomicU32>,
    fail: bool,
}
struct Prepared {
    committed: Arc<AtomicU32>,
    cancelled: Arc<AtomicU32>,
    did_commit: bool,
}
impl PreparedGraphFormat for Prepared {
    fn commit(mut self: Box<Self>) {
        self.committed.fetch_add(1, Ordering::Relaxed);
        self.did_commit = true;
    }
}
impl Drop for Prepared {
    fn drop(&mut self) {
        if !self.did_commit {
            self.cancelled.fetch_add(1, Ordering::Relaxed);
        }
    }
}
impl GraphResourceBackend for Resources {
    fn reconcile(
        &self,
        _: &GraphDocument,
        _: crate::PlaybackBindings<'_>,
    ) -> Result<(), HostAudioBackendError> {
        Ok(())
    }
    fn prepare_format_change(
        &self,
        _: &GraphDocument,
        _: crate::PlaybackBindings<'_>,
        _: RenderFormat,
        _: RenderFormat,
    ) -> Result<Box<dyn PreparedGraphFormat>, HostAudioBackendError> {
        if self.fail {
            return Err(failure());
        }
        Ok(Box::new(Prepared {
            committed: self.committed.clone(),
            cancelled: self.cancelled.clone(),
            did_commit: false,
        }))
    }
}

struct RejectNative(Resources);
struct RejectPrepared(Prepared);
impl PreparedGraphFormat for RejectPrepared {
    fn configure_native(&mut self) -> Result<(), HostAudioBackendError> {
        Err(failure())
    }
    fn commit(self: Box<Self>) {
        Box::new(self.0).commit();
    }
}
impl GraphResourceBackend for RejectNative {
    fn reconcile(
        &self,
        _: &GraphDocument,
        _: crate::PlaybackBindings<'_>,
    ) -> Result<(), HostAudioBackendError> {
        Ok(())
    }
    fn prepare_format_change(
        &self,
        _: &GraphDocument,
        _: crate::PlaybackBindings<'_>,
        _: RenderFormat,
        _: RenderFormat,
    ) -> Result<Box<dyn PreparedGraphFormat>, HostAudioBackendError> {
        Ok(Box::new(RejectPrepared(Prepared {
            committed: self.0.committed.clone(),
            cancelled: self.0.cancelled.clone(),
            did_commit: false,
        })))
    }
}

#[test]
fn resource_buffer_failure_rolls_back_session_without_committing_plan_or_resources() {
    let (mut controller, runtime) = fixture();
    let committed = Arc::new(AtomicU32::new(0));
    let cancelled = Arc::new(AtomicU32::new(0));
    controller.graph_resources = Arc::new(RejectNative(Resources {
        committed: committed.clone(),
        cancelled: cancelled.clone(),
        fail: false,
    }));
    let before = controller.graph();
    let current = controller.context.read().format;
    let requested = RenderFormat {
        sample_rate: 44_100,
        block_frames: 64,
    };
    assert!(
        controller
            .change_session(SessionReconfigureRequest {
                expected_revision: before.revision,
                format: mixrelay_audio_io::session::SessionFormat {
                    sample_rate: requested.sample_rate,
                    block_frames: requested.block_frames,
                },
            })
            .is_err()
    );
    assert_eq!(*runtime.negotiated.lock(), [requested, current]);
    assert_eq!(controller.graph(), before);
    assert_eq!(controller.context.read().format, current);
    assert_eq!(committed.load(Ordering::Acquire), 0);
    assert_eq!(cancelled.load(Ordering::Acquire), 1);
    let revisions = controller.exchange.revisions();
    assert_eq!(revisions.active, revisions.submitted);
}

#[test]
fn preparation_failure_or_unknown_rate_never_negotiates_native_format() {
    let (mut controller, runtime) = fixture();
    controller.graph_resources = Arc::new(Resources {
        committed: Arc::new(AtomicU32::new(0)),
        cancelled: Arc::new(AtomicU32::new(0)),
        fail: true,
    });
    assert!(controller.reconcile_observed_device_rate().is_err());
    assert_eq!(runtime.adoptions.load(Ordering::Acquire), 0);
    runtime.rate.store(0, Ordering::Release);
    assert!(
        controller
            .reconcile_observed_device_rate()
            .unwrap()
            .is_none()
    );
    runtime.rate.store(192_000, Ordering::Release);
    assert_eq!(
        controller
            .reconcile_observed_device_rate()
            .unwrap_err()
            .code,
        ErrorCode::AudioSessionUnsupported
    );
    assert_eq!(controller.graph().revision, 1);
}

#[test]
fn format_change_never_rebuilds_hosted_state_from_a_stale_document() {
    let (controller, runtime) = fixture();
    controller.graph.write().nodes.push(NodeSpec {
        control: mixrelay_audio_graph::NodeControl::default(),
        id: NodeId::from("missing-native-instance"),
        label: None,
        config: NodeConfig::Provider(mixrelay_audio_graph::ProviderNodeConfig {
            ports: None,
            type_id: "fixture:effect".into(),
            parameters: serde_json::json!({"nativeToken": "stale-client-preset"}),
        }),
    });
    let before = controller.graph();
    assert!(controller.reconcile_observed_device_rate().is_err());
    assert_eq!(controller.graph(), before);
    assert_eq!(controller.context.read().format.sample_rate, 48_000);
    assert_eq!(runtime.adoptions.load(Ordering::Acquire), 0);
}

#[test]
fn external_buffer_only_change_preserves_pending_and_stale_revision_guards() {
    let (controller, runtime) = fixture();
    runtime.rate.store(48_000, Ordering::Release);
    runtime.frames.store(256, Ordering::Release);
    let changed = controller
        .reconcile_observed_device_rate()
        .unwrap()
        .unwrap();
    assert_eq!(changed.revision, 2);
    assert_eq!(controller.context.read().format.block_frames, 256);
    runtime.frames.store(64, Ordering::Release);
    assert!(
        controller
            .reconcile_observed_device_rate()
            .unwrap()
            .is_none()
    );
    let stale = controller
        .change_session(SessionReconfigureRequest {
            expected_revision: 1,
            format: mixrelay_audio_io::session::SessionFormat {
                sample_rate: 48_000,
                block_frames: 128,
            },
        })
        .unwrap_err();
    assert_eq!(stale.code, ErrorCode::GraphRevisionStale);
    settle(&controller);
    assert_eq!(
        controller
            .reconcile_observed_device_rate()
            .unwrap()
            .unwrap()
            .revision,
        3
    );
    assert_eq!(controller.context.read().format.block_frames, 64);
    assert_eq!(runtime.changes.load(Ordering::Acquire), 0);
}

#[test]
fn native_rejection_cancels_staged_resources_without_publishing_graph() {
    let (mut controller, runtime) = fixture();
    let directory = crate::test_support::Documents::new("format-catalog-rollback");
    let path = directory.join("graphs.json");
    let committed = Arc::new(AtomicU32::new(0));
    let cancelled = Arc::new(AtomicU32::new(0));
    controller.graph_resources = Arc::new(Resources {
        committed: committed.clone(),
        cancelled: cancelled.clone(),
        fail: false,
    });
    let controller = controller
        .with_test_documents(&directory.files)
        .with_graph_catalog_path(path.clone());
    let saved = crate::test_support::read_document(&directory.files, &path).unwrap();
    runtime.fail.store(true, Ordering::Release);
    assert!(controller.reconcile_observed_device_rate().is_err());
    assert_eq!(controller.graph().revision, 1);
    assert_eq!(controller.context.read().format.sample_rate, 48_000);
    assert_eq!(cancelled.load(Ordering::Acquire), 1);
    assert_eq!(committed.load(Ordering::Acquire), 0);
    assert_eq!(
        crate::test_support::read_document(&directory.files, &path).unwrap(),
        saved
    );
    assert_eq!(directory.files.accepted_document_count(), 1);
    runtime.fail.store(false, Ordering::Release);
    assert!(
        controller
            .reconcile_observed_device_rate()
            .unwrap()
            .is_some()
    );
    assert_eq!(committed.load(Ordering::Acquire), 1);
    assert_ne!(
        crate::test_support::read_document(&directory.files, &path).unwrap(),
        saved
    );
    assert_eq!(directory.files.accepted_document_count(), 1);
}

#[test]
fn user_format_change_with_unwritable_catalog_does_not_prepare_native_resources() {
    let (mut controller, runtime) = fixture();
    let fixture = crate::test_support::Documents::new("user-rate-catalog");
    let committed = Arc::new(AtomicU32::new(0));
    let cancelled = Arc::new(AtomicU32::new(0));
    controller.graph_resources = Arc::new(Resources {
        committed: Arc::clone(&committed),
        cancelled: Arc::clone(&cancelled),
        fail: false,
    });
    let path = fixture.join("graphs.json");
    let controller = controller
        .with_test_documents(&fixture.files)
        .with_graph_catalog_path(path.clone());
    let saved = crate::test_support::read_document(&fixture.files, &path).unwrap();
    let before = controller.graph();
    let current = controller.context.read().format;
    fixture
        .files
        .reject_stage(path.clone(), std::io::ErrorKind::NotADirectory);

    assert!(
        controller
            .change_session(SessionReconfigureRequest {
                expected_revision: before.revision,
                format: mixrelay_audio_io::session::SessionFormat {
                    sample_rate: 44_100,
                    block_frames: 64,
                },
            })
            .is_err()
    );

    assert_eq!(cancelled.load(Ordering::Acquire), 0);
    assert_eq!(committed.load(Ordering::Acquire), 0);
    assert_eq!(runtime.changes.load(Ordering::Acquire), 0);
    assert_eq!(controller.graph(), before);
    assert_eq!(controller.context.read().format, current);
    assert_eq!(
        crate::test_support::read_document(&fixture.files, &path).unwrap(),
        saved
    );
}

#[test]
fn external_format_adopts_and_warns_when_graph_catalog_staging_fails() {
    let (controller, runtime) = fixture();
    let fixture = crate::test_support::Documents::new("external-rate-catalog");
    let directory = fixture.join("catalog");
    let path = directory.join("graphs.json");
    let controller = controller
        .with_test_documents(&fixture.files)
        .with_graph_catalog_path(path.clone());
    let saved = crate::test_support::read_document(&fixture.files, &path).unwrap();
    fixture
        .files
        .reject_stage(path.clone(), std::io::ErrorKind::NotADirectory);
    let response = controller
        .reconcile_observed_device_rate()
        .unwrap()
        .unwrap();
    assert!(response.persistence_error.is_some());
    assert_eq!(controller.context.read().format.sample_rate, 44_100);
    assert_eq!(runtime.adoptions.load(Ordering::Acquire), 1);
    assert_eq!(
        crate::test_support::read_document(&fixture.files, &path).unwrap(),
        saved
    );
    assert_eq!(controller.graph().revision, response.revision);
}
