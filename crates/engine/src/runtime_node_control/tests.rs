use super::*;
use crate::{DriverState, FixedRuntimeStatus, GraphResourceBackend};
use mixrelay_audio_engine::BlockProcessor;
use mixrelay_audio_graph::{GraphDocument, NodeSpec};
use mixrelay_audio_io::{
    ApplicationCaptureRequest, ApplicationSelector, ChannelLayout, HostAudioBackendError,
    SourcePlaybackPolicy, StreamSpec,
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct Apps {
    resolves: AtomicUsize,
    reconciles: AtomicUsize,
    unavailable: bool,
}

impl GraphResourceBackend for Apps {
    fn reconcile(
        &self,
        _: &GraphDocument,
        _: crate::PlaybackBindings<'_>,
    ) -> Result<(), HostAudioBackendError> {
        self.reconciles.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn resolve_application_capture(
        &self,
        request: &ApplicationCaptureRequest,
    ) -> Result<crate::graph_composition::SharedSource, HostAudioBackendError> {
        self.resolves.fetch_add(1, Ordering::Relaxed);
        if self.unavailable {
            return Err(HostAudioBackendError::BackendFailure {
                message: "fixture application is unavailable".into(),
            });
        }
        let mut canonical = request.clone();
        canonical.gain_db = 0.0;
        Ok(crate::graph_composition::SharedSource {
            id: "shared-fixture-application".into(),
            config: NodeConfig::ApplicationCapture(canonical),
        })
    }
}

fn graph(enabled: bool) -> GraphDocument {
    GraphDocument::new(
        1,
        vec![NodeSpec {
            id: "app".into(),
            control: NodeControl {
                enabled,
                muted: false,
            },
            label: None,
            config: NodeConfig::ApplicationCapture(ApplicationCaptureRequest {
                selector: ApplicationSelector::BundleId {
                    bundle_id: "io.mixrelay.fixture".into(),
                    include_descendants: true,
                },
                stream: StreamSpec {
                    channels: 2,
                    channel_layout: ChannelLayout::Stereo,
                },
                source_playback: SourcePlaybackPolicy::Preserve,
                forwarding_endpoint: None,
                gain_db: -6.0,
                requires_reselection: false,
            }),
        }],
        vec![],
    )
}

fn controller(
    directory: &crate::test_support::Documents,
    documents: &[(String, GraphDocument)],
    apps: &Arc<Apps>,
) -> EngineGraphController {
    let path = directory.join("graphs.json");
    let graphs: Vec<_> = documents
        .iter()
        .map(|(id, document)| serde_json::json!({"id":id,"name":id,"document":document,"layoutRevision":0,"layout":{"positions":{}}}))
        .collect();
    directory.files.insert(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "version":1,"revision":1,"activeId":documents[0].0,"graphs":graphs,
        }))
        .unwrap(),
    );
    let mut engine =
        crate::endpoint_settings_tests::controller(GraphDocument::new(1, vec![], vec![]));
    engine.runtime_status = Arc::new(FixedRuntimeStatus::new(DriverState::Ready));
    engine.graph_resources = apps.clone();
    engine
        .with_test_documents(&directory.files)
        .with_graph_catalog_path(path)
}

fn request(engine: &EngineGraphController, enabled: bool) -> NodeControlRequest {
    NodeControlRequest {
        expected_revision: engine.exchange.revisions().submitted,
        enabled: Some(enabled),
        muted: None,
        gain_db: None,
    }
}

fn acknowledge(engine: &EngineGraphController, processor: &mut BlockProcessor) {
    let inputs = vec![vec![0.25; 256]; processor.plan().source_bindings().len()];
    let slices: Vec<_> = inputs.iter().map(Vec::as_slice).collect();
    processor.process_block(128, &slices, &mut []).unwrap();
    assert_eq!(
        engine.exchange.revisions().active,
        engine.exchange.revisions().submitted
    );
}

#[test]
fn shared_app_power_manifests_follow_all_references_without_recomposition() {
    let directory = crate::test_support::Documents::new("shared-app-runtime-controls");
    let apps = Arc::new(Apps::default());
    let engine = controller(
        &directory,
        &[("a".into(), graph(true)), ("b".into(), graph(true))],
        &apps,
    );
    let mut processor = crate::controller_test_support::settle(&engine);
    let topology = engine.exchange.plan_revisions();
    let resolved = apps.resolves.load(Ordering::Relaxed);
    let reconciled = apps.reconciles.load(Ordering::Relaxed);
    let initial = engine.application_capture_manifest().unwrap();
    assert_eq!(initial.sources.len(), 1);
    let owner = NodeId::from(initial.sources[0].node_id.as_str());
    for (graph_id, enabled, remaining) in [("a", false, 1), ("b", false, 0), ("a", true, 1)] {
        let patch = request(&engine, enabled);
        let preview = engine.preview_node_switch(graph_id, "app", &patch).unwrap();
        let manifest = preview
            .capture_manifest
            .expect("application power has capture preflight");
        assert_eq!(manifest.sources.len(), remaining);
        if remaining != 0 {
            assert_eq!(manifest.sources[0], initial.sources[0]);
        }
        let response = engine.change_node_control(graph_id, "app", &patch).unwrap();
        assert!(!response.requires_graph_reload);
        acknowledge(&engine, &mut processor);
        assert_eq!(engine.application_capture_manifest().unwrap(), manifest);
        assert_eq!(
            processor
                .plan()
                .controls()
                .snapshot(&owner)
                .unwrap()
                .0
                .enabled,
            remaining != 0
        );
        assert_eq!(engine.exchange.plan_revisions(), topology);
        assert_eq!(apps.resolves.load(Ordering::Relaxed), resolved);
        assert_eq!(apps.reconciles.load(Ordering::Relaxed), reconciled);
    }
}

#[test]
fn initially_off_unavailable_app_does_not_resolve_until_first_on() {
    let directory = crate::test_support::Documents::new("dormant-app-runtime-controls");
    let apps = Arc::new(Apps {
        unavailable: true,
        ..Apps::default()
    });
    let original = graph(false);
    let engine = controller(&directory, &[("a".into(), original.clone())], &apps);
    let mut processor = crate::controller_test_support::settle(&engine);
    let topology = engine.exchange.plan_revisions();
    assert_eq!(apps.resolves.load(Ordering::Relaxed), 0);
    assert!(
        engine
            .application_capture_manifest()
            .unwrap()
            .sources
            .is_empty()
    );
    let mut muted = request(&engine, false);
    muted.enabled = None;
    muted.muted = Some(true);
    assert!(
        engine
            .preview_node_switch("a", "app", &muted)
            .unwrap()
            .capture_manifest
            .is_none()
    );
    assert!(
        !engine
            .change_node_control("a", "app", &muted)
            .unwrap()
            .requires_graph_reload
    );
    acknowledge(&engine, &mut processor);
    assert_eq!(apps.resolves.load(Ordering::Relaxed), 0);
    let before = engine.graph();
    let on = request(&engine, true);
    assert!(engine.preview_node_switch("a", "app", &on).is_err());
    assert_eq!(apps.resolves.load(Ordering::Relaxed), 1);
    assert!(engine.change_node_control("a", "app", &on).is_err());
    assert_eq!(apps.resolves.load(Ordering::Relaxed), 2);
    assert_eq!(engine.graph(), before);
    assert_eq!(engine.exchange.plan_revisions(), topology);
    let NodeConfig::ApplicationCapture(authored) = &before.nodes[0].config else {
        panic!("authored application identity must remain intact")
    };
    assert!(!authored.requires_reselection);
}
