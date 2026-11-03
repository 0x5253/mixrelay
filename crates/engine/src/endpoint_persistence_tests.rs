use super::*;
use crate::controller_test_support::settle;
use crate::endpoint_settings_tests::{controller_with_inventory, endpoint_document, settings};
use mixrelay_audio_io::VirtualEndpointSettings;
use mixrelay_control_protocol::{GraphCatalogAction, GraphCatalogRequest};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

struct Fixture {
    directory: crate::test_support::Documents,
    inventory: GraphDocument,
    reject_attachments: Arc<AtomicBool>,
}

struct AttachmentBackend {
    base: Arc<dyn VirtualEndpointBackend>,
    reject: Arc<AtomicBool>,
}

impl VirtualEndpointBackend for AttachmentBackend {
    fn capabilities(&self) -> &VirtualEndpointCapabilities {
        self.base.capabilities()
    }

    fn settings(&self) -> Option<VirtualEndpointSettings> {
        self.base.settings()
    }

    fn resolve_graph_devices(
        &self,
        desired: &[VirtualEndpointSpec],
    ) -> Result<Vec<VirtualEndpointSpec>, HostAudioBackendError> {
        self.base.resolve_graph_devices(desired)
    }

    fn prepare_graph_attachments(
        &self,
        desired: &[VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError> {
        if self.reject.load(Ordering::Acquire) {
            return Err(HostAudioBackendError::BackendFailure {
                message: "fixture attachment staging rejected".into(),
            });
        }
        self.base.prepare_graph_attachments(desired)
    }
}

impl Fixture {
    fn new(name: &str) -> Self {
        Self {
            directory: crate::test_support::Documents::new("endpoint-catalog"),
            inventory: endpoint_document(1, name),
            reject_attachments: Arc::new(AtomicBool::new(false)),
        }
    }

    fn path(&self) -> PathBuf {
        self.directory.join("graphs.json")
    }

    fn restart(&self) -> EngineGraphController {
        let mut engine =
            controller_with_inventory(GraphDocument::new(1, vec![], vec![]), &self.inventory);
        engine.endpoint_backend = Arc::new(AttachmentBackend {
            base: engine.endpoint_backend.clone(),
            reject: self.reject_attachments.clone(),
        });
        engine
            .with_test_documents(&self.directory.files)
            .with_graph_catalog_path(self.path())
    }
}

fn select_view(controller: &EngineGraphController, action: GraphCatalogAction) {
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

#[test]
fn catalog_restart_restores_device_format_name_and_graph_local_reference() {
    let fixture = Fixture::new("SPAT send-return");
    let first = fixture.restart();
    let mut document = endpoint_document(2, "SPAT send-return");
    let NodeConfig::VirtualEndpoint(config) = &mut document.nodes[0].config else {
        panic!("endpoint fixture")
    };
    config.reference.capture = false;
    config.gain_db = -6.0;
    first.apply_graph_request(document.clone().into()).unwrap();
    let expected = settings(&first).devices;
    drop(first);

    let next = fixture.restart();
    assert_eq!(settings(&next).devices, expected);
    assert_eq!(next.graph().nodes, document.nodes);
    assert!(next.graph().edges.is_empty());
    assert_eq!(next.graph().revision, 3);
    assert_eq!(next.graph_status().active_revision, 1);
    settle(&next);
    assert_eq!(next.graph_status().active_revision, 3);
    assert_eq!(fixture.directory.files.accepted_document_count(), 1);
}

#[test]
fn final_reference_removal_persists_without_recreating_a_device_on_restart() {
    let fixture = Fixture::new("Temporary device");
    let first = fixture.restart();
    let template = settings(&first).creation_template;
    let independent_devices = settings(&first).devices;
    first
        .apply_graph_request(endpoint_document(2, "Temporary device").into())
        .unwrap();
    settle(&first);
    first
        .apply_graph_request(GraphDocument::new(3, vec![], vec![]).into())
        .unwrap();
    drop(first);

    let next = fixture.restart();
    assert!(next.graph().nodes.is_empty());
    assert_eq!(settings(&next).devices, independent_devices);
    assert!(settings(&next).can_create);
    assert_eq!(settings(&next).creation_template, template);
    settle(&next);
    drop(next);
    assert_eq!(settings(&fixture.restart()).devices, independent_devices);
}

#[test]
fn driver_lifetime_is_independent_of_references_in_every_saved_graph_across_restart() {
    let fixture = Fixture::new("Shared device");
    let engine = fixture.restart();
    let independent_devices = settings(&engine).devices;
    engine
        .apply_graph_request(endpoint_document(2, "Shared device").into())
        .unwrap();
    settle(&engine);
    let first_id = engine.graph_catalog().unwrap().active_id;
    select_view(
        &engine,
        GraphCatalogAction::Create {
            name: "Second view".into(),
        },
    );
    assert!(engine.graph().nodes.is_empty());
    assert_eq!(settings(&engine).devices.len(), 1);
    engine
        .apply_graph_request(endpoint_document(engine.graph().revision + 1, "Shared device").into())
        .unwrap();
    settle(&engine);
    engine
        .apply_graph_request(GraphDocument::new(engine.graph().revision + 1, vec![], vec![]).into())
        .unwrap();
    assert_eq!(settings(&engine).devices.len(), 1);
    drop(engine);

    let restarted = fixture.restart();
    assert!(restarted.graph().nodes.is_empty());
    assert_eq!(settings(&restarted).devices.len(), 1);
    settle(&restarted);
    select_view(&restarted, GraphCatalogAction::Select { id: first_id });
    restarted
        .apply_graph_request(
            GraphDocument::new(restarted.graph().revision + 1, vec![], vec![]).into(),
        )
        .unwrap();
    assert_eq!(settings(&restarted).devices, independent_devices);
    drop(restarted);
    let final_restart = fixture.restart();
    assert_eq!(settings(&final_restart).devices, independent_devices);
    assert!(settings(&final_restart).can_create);
    assert_eq!(final_restart.graph_catalog().unwrap().graphs.len(), 2);
}

#[test]
fn native_staging_rejection_preserves_catalog_bytes_and_current_device() {
    let fixture = Fixture::new("Accepted");
    let first = fixture.restart();
    first
        .apply_graph_request(endpoint_document(2, "Accepted").into())
        .unwrap();
    settle(&first);
    let original =
        crate::test_support::read_document(&fixture.directory.files, &fixture.path()).unwrap();
    let expected = first.graph();
    let independent_devices = settings(&first).devices;
    fixture.reject_attachments.store(true, Ordering::Release);
    assert!(
        first
            .apply_graph_request(endpoint_document(3, "rejected").into())
            .is_err()
    );
    assert_eq!(first.graph(), expected);
    assert_eq!(settings(&first).devices, independent_devices);
    assert_eq!(
        crate::test_support::read_document(&fixture.directory.files, &fixture.path()).unwrap(),
        original
    );
    assert_eq!(fixture.directory.files.accepted_document_count(), 1);
    fixture.reject_attachments.store(false, Ordering::Release);
    drop(first);
    assert_eq!(fixture.restart().graph().nodes, expected.nodes);
}

#[test]
fn catalog_staging_failure_preserves_disk_memory_and_allows_retry() {
    let fixture = Fixture::new("Accepted");
    let first = fixture.restart();
    first
        .apply_graph_request(endpoint_document(2, "Accepted").into())
        .unwrap();
    settle(&first);
    let original =
        crate::test_support::read_document(&fixture.directory.files, &fixture.path()).unwrap();
    let expected = first.graph();
    fixture
        .directory
        .files
        .reject_stage(fixture.path(), std::io::ErrorKind::NotADirectory);

    let error = first
        .apply_graph_request(endpoint_document(3, "Unsaved").into())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ControlUnavailable);
    assert_eq!(first.graph(), expected);
    assert_eq!(first.graph_catalog().unwrap().document, expected);
    assert_eq!(
        crate::test_support::read_document(&fixture.directory.files, &fixture.path()).unwrap(),
        original
    );
    fixture.directory.files.allow_stage(&fixture.path());
    first
        .apply_graph_request(endpoint_document(3, "Recovered").into())
        .unwrap();
    drop(first);
    let restarted = fixture.restart();
    assert_eq!(
        settings(&restarted).devices[0].spec.display_name,
        "Accepted"
    );
    assert_eq!(
        restarted.graph().nodes[0].label.as_deref(),
        Some("Recovered")
    );
}

#[test]
fn corrupt_catalog_is_untouched_and_reports_an_observable_error() {
    let fixture = Fixture::new("Volatile device");
    let original = b"{broken catalog";
    fixture.directory.files.insert(fixture.path(), original);
    let engine = fixture.restart();
    assert!(engine.graph().nodes.is_empty());
    let error = engine.graph_catalog().unwrap_err();
    assert_eq!(error.code, ErrorCode::ControlUnavailable);
    assert_eq!(error.parameters["resource"], "graph_catalog");
    assert!(!error.parameters["reason"].as_str().unwrap().is_empty());
    engine
        .apply_graph_request(endpoint_document(2, "Volatile device").into())
        .unwrap();
    assert!(engine.graph_catalog().is_err());
    assert_eq!(
        crate::test_support::read_document(&fixture.directory.files, &fixture.path()).unwrap(),
        original
    );
    drop(engine);
    assert!(fixture.restart().graph_catalog().is_err());
    assert_eq!(fixture.directory.files.accepted_document_count(), 1);
}

#[test]
fn failed_native_restore_keeps_saved_catalog_and_defers_native_preparation() {
    let fixture = Fixture::new("Accepted");
    let engine = fixture.restart();
    engine
        .apply_graph_request(endpoint_document(2, "Accepted").into())
        .unwrap();
    let saved_document = engine.graph();
    let independent_devices = settings(&engine).devices;
    drop(engine);
    let original =
        crate::test_support::read_document(&fixture.directory.files, &fixture.path()).unwrap();
    fixture.reject_attachments.store(true, Ordering::Release);
    let restarted = fixture.restart();
    assert_eq!(restarted.graph(), saved_document);
    assert_eq!(
        restarted.graph_catalog().unwrap().document,
        restarted.graph()
    );
    assert!(restarted.catalog_restore.is_pending());
    assert_eq!(restarted.exchange.revisions().active, 1);
    assert_eq!(restarted.exchange.revisions().submitted, 1);
    assert_eq!(
        crate::test_support::read_document(&fixture.directory.files, &fixture.path()).unwrap(),
        original
    );
    assert_eq!(settings(&restarted).devices, independent_devices);
}
