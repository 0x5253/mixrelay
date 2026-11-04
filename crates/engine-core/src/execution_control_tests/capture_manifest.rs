//! Application capture manifests read without waiting on native identity probes.
use super::*;

#[derive(Default)]
struct ManifestNativeProbe {
    inventories: std::sync::atomic::AtomicUsize,
    application_resolutions: std::sync::atomic::AtomicUsize,
}

impl GraphResourceBackend for ManifestNativeProbe {
    fn device_inventory(
        &self,
        _: &[&mixrelay_audio_io::DeviceSelector],
    ) -> Result<
        Vec<mixrelay_audio_io::AudioDeviceDescriptor>,
        mixrelay_audio_io::HostAudioBackendError,
    > {
        self.inventories
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Vec::new())
    }

    fn resolve_application_capture(
        &self,
        request: &mixrelay_audio_io::ApplicationCaptureRequest,
    ) -> Result<crate::graph_composition::SharedSource, mixrelay_audio_io::HostAudioBackendError>
    {
        self.application_resolutions
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut canonical = request.clone();
        canonical.gain_db = 0.0;
        Ok(crate::graph_composition::SharedSource {
            id: "io.mixrelay.manifest-fixture".into(),
            config: NodeConfig::ApplicationCapture(canonical),
        })
    }

    fn reconcile(
        &self,
        _: &GraphDocument,
        _: crate::PlaybackBindings<'_>,
    ) -> Result<(), mixrelay_audio_io::HostAudioBackendError> {
        Ok(())
    }
}

fn manifest_application_graph(revision: u64) -> GraphDocument {
    GraphDocument::new(
        revision,
        vec![NodeSpec {
            id: "app".into(),
            control: mixrelay_audio_graph::NodeControl::default(),
            label: None,
            config: NodeConfig::ApplicationCapture(mixrelay_audio_io::ApplicationCaptureRequest {
                selector: mixrelay_audio_io::ApplicationSelector::BundleId {
                    bundle_id: "io.mixrelay.manifest-fixture".into(),
                    include_descendants: false,
                },
                stream: StreamSpec {
                    channels: 2,
                    channel_layout: ChannelLayout::Stereo,
                },
                source_playback: mixrelay_audio_io::SourcePlaybackPolicy::Preserve,
                forwarding_endpoint: None,
                gain_db: -6.0,
                requires_reselection: false,
            }),
        }],
        vec![],
    )
}

#[test]
fn manifest_read_returns_unavailable_without_waiting_for_native_writer() {
    use std::{sync::Arc, sync::mpsc, thread, time::Duration};

    let engine = Arc::new(crate::endpoint_settings_tests::controller(
        GraphDocument::new(1, vec![], vec![]),
    ));
    let _prepared = engine
        .prepare_and_validate(&engine.graph(), *engine.context.read())
        .unwrap();
    let guard = engine.mutation.lock();
    let (sender, receiver) = mpsc::channel();
    let reader = Arc::clone(&engine);
    let worker = thread::spawn(move || sender.send(reader.current_capture_manifest()).unwrap());
    let result = receiver.recv_timeout(Duration::from_secs(1));
    drop(guard);
    worker.join().unwrap();
    let error = result.unwrap().unwrap_err();
    assert_eq!(error.code, ErrorCode::ControlUnavailable);
    assert_eq!(error.parameters["resource"], "application_capture_manifest");
    assert_eq!(error.parameters["reason"], "graph_mutation_in_progress");
}

#[test]
fn empty_bootstrap_manifest_is_available_without_native_identity_probes() {
    use std::sync::{Arc, atomic::Ordering};

    let probe = Arc::new(ManifestNativeProbe::default());
    let mut engine =
        crate::endpoint_settings_tests::controller(GraphDocument::new(1, vec![], vec![]));
    engine.graph_resources = probe.clone();
    assert!(engine.execution_views.read().is_empty());
    let manifest = engine.current_capture_manifest().unwrap();
    assert_eq!(manifest.revision, engine.exchange.revisions().submitted);
    assert!(manifest.sources.is_empty());
    assert_eq!(probe.inventories.load(Ordering::Relaxed), 0);
    assert_eq!(probe.application_resolutions.load(Ordering::Relaxed), 0);
    // An authored graph at a revision not represented by the bootstrap processor
    // cannot masquerade as an empty execution snapshot.
    engine.graph.write().revision += 1;
    assert!(engine.current_capture_manifest().is_err());
}

#[test]
fn unpublished_manifest_never_reprobes_physical_or_application_identity() {
    use std::sync::{Arc, atomic::Ordering};

    let mut document = manifest_application_graph(1);
    document.nodes.push(NodeSpec {
        id: "microphone".into(),
        control: mixrelay_audio_graph::NodeControl::default(),
        label: None,
        config: NodeConfig::AudioDeviceInput(mixrelay_audio_graph::AudioDeviceInputConfig {
            selector: mixrelay_audio_io::DeviceSelector::DefaultInput,
            output_channels: 2,
            gain_db: 0.0,
        }),
    });
    let probe = Arc::new(ManifestNativeProbe::default());
    let mut engine = crate::endpoint_settings_tests::controller(document);
    engine.runtime_status = Arc::new(crate::FixedRuntimeStatus::new(crate::DriverState::Ready));
    engine.graph_resources = probe.clone();
    assert!(engine.execution_views.read().is_empty());
    let error = engine.current_capture_manifest().unwrap_err();
    assert_eq!(error.code, ErrorCode::ControlUnavailable);
    assert_eq!(error.parameters["resource"], "application_capture_manifest");
    assert_eq!(
        error.parameters["reason"],
        "execution_snapshot_not_published"
    );
    assert_eq!(probe.inventories.load(Ordering::Relaxed), 0);
    assert_eq!(probe.application_resolutions.load(Ordering::Relaxed), 0);
}

fn published_manifest_controller() -> (
    EngineGraphController,
    std::sync::Arc<ManifestNativeProbe>,
    crate::test_support::Documents,
) {
    use std::sync::Arc;
    let directory = crate::test_support::Documents::new("published-capture-manifest");
    let path = directory.join("graphs.json");
    let document = manifest_application_graph(1);
    directory.files.insert(
        path.clone(),
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "revision": 1,
            "activeId": "first",
            "graphs": [
                {"id":"first","name":"First","document":document,"layoutRevision":0,"layout":{"positions":{}}},
                {"id":"second","name":"Second","document":document,"layoutRevision":0,"layout":{"positions":{}}},
            ],
        }))
        .unwrap(),
    );
    let probe = Arc::new(ManifestNativeProbe::default());
    let mut engine =
        crate::endpoint_settings_tests::controller(GraphDocument::new(1, vec![], vec![]));
    engine.runtime_status = Arc::new(crate::FixedRuntimeStatus::new(crate::DriverState::Ready));
    engine.graph_resources = probe.clone();
    (
        directory.bind(engine).with_graph_catalog_path(path),
        probe,
        directory,
    )
}

fn acknowledge_manifest_controls(engine: &EngineGraphController, processor: &mut BlockProcessor) {
    let inputs = vec![vec![0.0; 256]; processor.plan().source_bindings().len()];
    let slices: Vec<_> = inputs.iter().map(Vec::as_slice).collect();
    processor.process_block(128, &slices, &mut []).unwrap();
    assert_eq!(
        engine.exchange.revisions().active,
        engine.exchange.revisions().submitted
    );
}

#[test]
fn published_manifest_preserves_shared_canonical_owner_controls_and_power_revision() {
    use std::sync::atomic::Ordering;

    let (engine, probe, _directory) = published_manifest_controller();
    let mut processor = crate::controller_test_support::settle(&engine);
    let resolved = probe.application_resolutions.load(Ordering::Relaxed);
    let manifest = engine.current_capture_manifest().unwrap();
    assert_eq!(manifest.sources.len(), 1);
    let bundle_id = "io.mixrelay.manifest-fixture";
    let shared_id = format!("application:{}:{bundle_id}", bundle_id.len());
    assert_eq!(
        manifest.sources[0].node_id,
        format!("s:{}:{shared_id}:native", shared_id.len())
    );
    assert_eq!(
        manifest.sources[0].request.gain_db.to_bits(),
        0.0_f32.to_bits()
    );
    assert_eq!(manifest.revision, engine.exchange.revisions().submitted);
    assert_eq!(
        probe.application_resolutions.load(Ordering::Relaxed),
        resolved
    );

    for (graph_id, remaining) in [("first", 1), ("second", 0)] {
        let response = engine
            .change_node_control(
                graph_id,
                "app",
                &mixrelay_control_protocol::NodeControlRequest {
                    expected_revision: engine.exchange.revisions().submitted,
                    enabled: Some(false),
                    muted: None,
                    gain_db: None,
                },
            )
            .unwrap();
        acknowledge_manifest_controls(&engine, &mut processor);
        let current = engine.current_capture_manifest().unwrap();
        assert_eq!(current.revision, response.application.revision);
        assert_eq!(current.sources.len(), remaining);
        assert_eq!(
            probe.application_resolutions.load(Ordering::Relaxed),
            resolved
        );
    }

    let reenabled = engine
        .change_node_control(
            "first",
            "app",
            &mixrelay_control_protocol::NodeControlRequest {
                expected_revision: engine.exchange.revisions().submitted,
                enabled: Some(true),
                muted: None,
                gain_db: None,
            },
        )
        .unwrap();
    acknowledge_manifest_controls(&engine, &mut processor);
    let running = engine.current_capture_manifest().unwrap();
    assert_eq!(running.revision, reenabled.application.revision);
    assert_eq!(running.sources, manifest.sources);

    let off = engine.change_engine_power(false, None).unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    let stopped = engine.current_capture_manifest().unwrap();
    assert_eq!(stopped.revision, off.revision);
    assert!(stopped.sources.is_empty());
}

#[test]
fn playback_manifest_uses_live_masks_and_rejects_conflicting_active_targets() {
    use mixrelay_audio_graph::NodeControl;
    use mixrelay_audio_io::SourcePlaybackPolicy;
    let mut manifest = capture_manifest(&manifest_application_graph(7)).unwrap();
    manifest.sources[0].request.gain_db = 0.0;
    let capture = NodeId::from(manifest.sources[0].node_id.as_str());
    let intents: Vec<_> = [("first", DEVICE_A), ("second", DEVICE_B)]
        .into_iter()
        .map(
            |(mask, endpoint)| crate::graph_composition::ApplicationPlaybackIntent {
                source_id: "shared-app".into(),
                capture_node_id: capture.clone(),
                mask_node_id: mask.into(),
                forwarding_endpoint: endpoint.into(),
            },
        )
        .collect();
    let original = manifest.clone();
    assert!(
        apply_playback_intents(&mut manifest, &intents, |_| Some(NodeControl::default())).is_err()
    );
    let mut manifest = original;
    apply_playback_intents(&mut manifest, &intents, |id| {
        Some(NodeControl {
            enabled: true,
            muted: id.as_str() == "second",
        })
    })
    .unwrap();
    assert_eq!(
        manifest.sources[0].request.source_playback,
        SourcePlaybackPolicy::MuteWhileCaptured
    );
    assert_eq!(
        manifest.sources[0]
            .request
            .forwarding_endpoint
            .as_ref()
            .unwrap()
            .as_str(),
        DEVICE_A
    );
    apply_playback_intents(&mut manifest, &intents, |_| {
        Some(NodeControl {
            enabled: false,
            muted: false,
        })
    })
    .unwrap();
    assert_eq!(
        manifest.sources[0].request.source_playback,
        SourcePlaybackPolicy::Preserve
    );
    assert_eq!(manifest.sources[0].request.forwarding_endpoint, None);
    assert_eq!(
        manifest.sources[0].request.gain_db.to_bits(),
        0.0_f32.to_bits()
    );
}
