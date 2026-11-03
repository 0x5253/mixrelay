use super::super::{Catalog, GraphCatalogAction};
use super::require_pid_reselection;
use crate::GraphDocument;
use serde_json::{Value, json};
use std::path::PathBuf;

struct TemporaryNativeFailure(std::sync::atomic::AtomicUsize);

impl crate::GraphResourceBackend for TemporaryNativeFailure {
    fn reconcile(
        &self,
        _: &GraphDocument,
        _: crate::PlaybackBindings<'_>,
    ) -> Result<(), mixrelay_audio_io::HostAudioBackendError> {
        if self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
            return Err(mixrelay_audio_io::HostAudioBackendError::BackendFailure {
                message: "injected initialization timeout".into(),
            });
        }
        Ok(())
    }
}

#[test]
fn transient_native_startup_failure_retries_without_losing_saved_catalog_or_bootstrap_overwrite() {
    use crate::default_duplex_document;
    use std::sync::{Arc, atomic::Ordering};

    let saved = default_duplex_document(9, Some(-6.0));
    let other = default_duplex_document(12, Some(-12.0));
    let fixture = Fixture::new(&json!({
        "version":1,"revision":12,"activeId":"first",
        "graphs":[
            {"id":"first","name":"First","document":saved,"layoutRevision":0,"layout":{"positions":{}}},
            {"id":"second","name":"Second","document":other,"layoutRevision":0,"layout":{"positions":{}}}
        ]
    }));
    let before =
        crate::test_support::read_document(&fixture.documents.files, &fixture.path).unwrap();
    let backend = Arc::new(TemporaryNativeFailure(std::sync::atomic::AtomicUsize::new(
        0,
    )));
    let mut engine = crate::endpoint_settings_tests::controller(default_duplex_document(1, None));
    engine.graph_resources = backend.clone();
    let engine = fixture
        .documents
        .bind(engine)
        .with_graph_catalog_path(fixture.path.clone());
    assert_eq!(backend.0.load(Ordering::Relaxed), 1);
    assert_eq!(engine.graph(), saved);
    assert_eq!(engine.graph_catalog().unwrap().document, saved);
    assert_eq!(engine.graph_catalog_document("second").unwrap(), other);
    assert_eq!(
        crate::test_support::read_document(&fixture.documents.files, &fixture.path).unwrap(),
        before
    );
    let _status = engine.status();
    let due = engine.catalog_restore.deadline().unwrap();
    engine.retry_catalog_restore_at(mixrelay_time::Instant::from_origin(
        due.duration_since(mixrelay_time::Instant::from_origin(
            std::time::Duration::ZERO,
        ))
        .checked_sub(std::time::Duration::from_nanos(1))
        .unwrap(),
    ));
    assert_eq!(
        backend.0.load(Ordering::Relaxed),
        1,
        "reads and early ticks must not retry"
    );
    engine.retry_catalog_restore_at(due);
    assert_eq!(backend.0.load(Ordering::Relaxed), 2);
    assert!(!engine.catalog_restore.is_pending());
    assert_eq!(engine.graph().nodes, saved.nodes);
    assert_eq!(engine.graph().edges, saved.edges);
    assert_eq!(engine.graph().revision, 13);
    assert_eq!(engine.graph_catalog_document("second").unwrap(), other);
    assert_eq!(engine.graph_catalog().unwrap().active_id, "first");
    engine.retry_catalog_restore();
    assert_eq!(
        backend.0.load(Ordering::Relaxed),
        2,
        "successful recall must not run twice"
    );
}

struct Fixture {
    path: PathBuf,
    documents: crate::test_support::Documents,
}
impl Fixture {
    fn new(value: &Value) -> Self {
        let directory = crate::test_support::Documents::new("pid-restore");
        let path = directory.join("catalog.json");
        directory
            .files
            .insert(path.clone(), serde_json::to_vec(value).unwrap());
        Self {
            path,
            documents: directory,
        }
    }
}

fn document() -> GraphDocument {
    serde_json::from_value(json!({
        "schemaVersion": 2, "revision": 9,
        "nodes": [
            {"id":"pid", "label":"Saved application", "type":"application_capture",
             "config":{"selector":{"match":"process_id","process_id":4242,"include_descendants":true},
                "stream":{"channels":2,"channelLayout":"stereo"},"sourcePlayback":"preserve","gainDb":-6}},
            {"id":"path", "label":"Persistent path", "type":"application_capture",
             "config":{"selector":{"match":"executable_path","path":"/opt/audio/app"},
                "stream":{"channels":2,"channelLayout":"stereo"},"sourcePlayback":"preserve"}},
            {"id":"bundle", "label":"Persistent bundle", "type":"application_capture",
             "config":{"selector":{"match":"bundle_id","bundle_id":"io.example.audio","include_descendants":true},
                "stream":{"channels":2,"channelLayout":"stereo"},"sourcePlayback":"preserve"}},
            {"id":"gain", "label":"Output gain", "type":"gain", "config":{"channels":2,"gainDb":-3}}
        ],
        "edges":[{"id":"cable", "source":{"nodeId":"pid","portId":"audio"},
            "destination":{"nodeId":"gain","portId":"audio"},"channelMapping":{"mode":"auto"}}]
    })).unwrap()
}

fn catalog() -> Value {
    let document = document();
    json!({
        "version":1,"revision":12,"activeId":"first",
        "graphs":[
            {"id":"first","name":"First","document":document,"layoutRevision":0,"layout":{"positions":{}}},
            {"id":"second","name":"Second","document":document,"layoutRevision":0,"layout":{"positions":{}}}
        ]
    })
}

#[test]
fn loaded_active_and_inactive_pid_nodes_require_reselection_without_changing_saved_bytes() {
    let original = catalog();
    let fixture = Fixture::new(&original);
    let bytes =
        crate::test_support::read_document(&fixture.documents.files, &fixture.path).unwrap();
    let restored = Catalog::load(&fixture.documents.files, &fixture.path)
        .unwrap()
        .unwrap();
    let mut expected =
        serde_json::to_value(serde_json::from_value::<Catalog>(original).unwrap()).unwrap();
    for graph in expected["graphs"].as_array_mut().unwrap() {
        graph["document"]["nodes"][0]["config"]["requiresReselection"] = json!(true);
    }
    // Exact comparison preserves PID, labels, gains, cables, identities and revisions;
    // stable path/bundle requests receive no new restriction.
    assert_eq!(serde_json::to_value(restored).unwrap(), expected);
    assert_eq!(
        crate::test_support::read_document(&fixture.documents.files, &fixture.path).unwrap(),
        bytes
    );
}

#[test]
fn restored_catalog_remains_selectable_renamable_and_flag_survives_resaving() {
    let fixture = Fixture::new(&catalog());
    let mut restored = Catalog::load(&fixture.documents.files, &fixture.path)
        .unwrap()
        .unwrap();
    assert!(
        restored
            .apply_action(
                GraphCatalogAction::Select {
                    id: "second".into()
                },
                10,
                13
            )
            .unwrap()
    );
    assert!(
        !restored
            .apply_action(
                GraphCatalogAction::Rename {
                    id: "second".into(),
                    name: "Reselect source here".into(),
                },
                10,
                13
            )
            .unwrap()
    );
    let response = restored.response();
    assert_eq!(response.active_id, "second");
    assert_eq!(response.document.nodes.len(), 4);
    assert_eq!(response.document.edges, document().edges);
    let saved = Fixture::new(&serde_json::to_value(&restored).unwrap());
    let reloaded = Catalog::load(&saved.documents.files, &saved.path)
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(reloaded).unwrap(),
        serde_json::to_value(restored).unwrap()
    );
}

#[test]
fn fresh_pid_request_is_unrestricted_until_restore_and_marking_is_idempotent() {
    let fresh = document();
    let original = serde_json::to_value(&fresh).unwrap();
    assert!(
        original["nodes"][0]["config"]
            .get("requiresReselection")
            .is_none()
    );
    let mut restored = fresh;
    require_pid_reselection(&mut restored);
    let first = restored.clone();
    require_pid_reselection(&mut restored);
    assert_eq!(restored, first);
    // Re-adding an application creates a new request; it does not inherit the flag.
    let reselected = document();
    assert!(
        serde_json::to_value(reselected).unwrap()["nodes"][0]["config"]
            .get("requiresReselection")
            .is_none()
    );
}

#[test]
fn restored_pid_graph_can_be_edited_and_saved_through_controller() {
    use crate::{DriverState, FixedRuntimeStatus, NodeConfig};
    use std::sync::Arc;

    let fixture = Fixture::new(&catalog());
    let mut engine =
        crate::endpoint_settings_tests::controller(GraphDocument::new(1, vec![], vec![]));
    // This is a portable controller test with no native resource backend or device.
    engine.runtime_status = Arc::new(FixedRuntimeStatus::new(DriverState::Ready));
    engine.graph_resources = Arc::new(FixtureApplicationIdentities);
    let engine = fixture
        .documents
        .bind(engine)
        .with_graph_catalog_path(fixture.path.clone());
    assert!(engine.graph_catalog().unwrap().persistence_error.is_none());
    let mut edited = engine.graph();
    let NodeConfig::ApplicationCapture(request) = &edited.nodes[0].config else {
        panic!("saved PID node must remain editable");
    };
    assert!(request.requires_reselection);
    let manifest = engine.application_capture_manifest().unwrap();
    assert_eq!(manifest.sources.len(), 2);
    assert!(manifest.sources.iter().all(|source| !matches!(
        source.request.selector,
        mixrelay_audio_io::ApplicationSelector::ProcessId { .. }
    )));
    crate::controller_test_support::settle(&engine);
    edited.revision += 1;
    let NodeConfig::Gain(gain) = &mut edited.nodes[3].config else {
        panic!("saved gain node");
    };
    gain.gain_db = -12.0;
    engine.apply_graph_request(edited.clone().into()).unwrap();
    assert_eq!(engine.graph(), edited);
    let saved = Catalog::load(&fixture.documents.files, &fixture.path)
        .unwrap()
        .unwrap();
    assert_eq!(saved.active().document.nodes, edited.nodes);
    assert_eq!(saved.active().document.edges, edited.edges);
}

struct FixtureApplicationIdentities;

impl crate::GraphResourceBackend for FixtureApplicationIdentities {
    fn reconcile(
        &self,
        _: &GraphDocument,
        _: crate::PlaybackBindings<'_>,
    ) -> Result<(), mixrelay_audio_io::HostAudioBackendError> {
        Ok(())
    }

    fn resolve_application_capture(
        &self,
        request: &mixrelay_audio_io::ApplicationCaptureRequest,
    ) -> Result<crate::graph_composition::SharedSource, mixrelay_audio_io::HostAudioBackendError>
    {
        use mixrelay_audio_io::ApplicationSelector;
        let id = match &request.selector {
            ApplicationSelector::BundleId { .. } => "fixture-bundle",
            ApplicationSelector::ExecutablePath { .. } => "fixture-path",
            _ => panic!("restored PID must never reach native identity resolution"),
        };
        let mut canonical = request.clone();
        canonical.gain_db = 0.0;
        Ok(crate::graph_composition::SharedSource {
            id: id.into(),
            config: crate::NodeConfig::ApplicationCapture(canonical),
        })
    }
}
