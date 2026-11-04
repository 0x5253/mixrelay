//! Controller/document/DSP regression with a recording device-contract adapter.
//! This does not open the host's installed virtual device or claim native re-open evidence.
use super::*;
use crate::endpoint_settings_tests::controller;
use mixrelay_render::BlockProcessor;

#[derive(Default)]
struct RecordingEndpoint {
    contract: ContractOnlyVirtualEndpointBackend,
    current: parking_lot::Mutex<Vec<VirtualEndpointSpec>>,
    prepares: std::sync::atomic::AtomicUsize,
    changed_specs: std::sync::atomic::AtomicUsize,
    rate_changes: std::sync::atomic::AtomicUsize,
}

impl VirtualEndpointBackend for RecordingEndpoint {
    fn capabilities(&self) -> &VirtualEndpointCapabilities {
        self.contract.capabilities()
    }

    fn resolve_graph_devices(
        &self,
        desired: &[VirtualEndpointSpec],
    ) -> Result<Vec<VirtualEndpointSpec>, HostAudioBackendError> {
        let current = self.current.lock();
        desired
            .iter()
            .map(|reference| {
                current
                    .iter()
                    .find(|spec| spec.id == reference.id)
                    .cloned()
                    .ok_or_else(|| HostAudioBackendError::BackendFailure {
                        message: "Driver device is missing".into(),
                    })
            })
            .collect()
    }

    fn set_sample_rate(&self, _: &VirtualEndpointId, _: u32) -> Result<(), HostAudioBackendError> {
        self.rate_changes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err(HostAudioBackendError::BackendFailure {
            message: "graph cannot retune Driver".into(),
        })
    }

    fn prepare_graph_attachments(
        &self,
        desired: &[VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError> {
        use std::sync::atomic::Ordering;
        self.prepares.fetch_add(1, Ordering::Relaxed);
        let current = self.current.lock();
        if *current != desired {
            self.changed_specs.fetch_add(1, Ordering::Relaxed);
            return Err(HostAudioBackendError::BackendFailure {
                message: "graph cannot change Driver geometry or rate".into(),
            });
        }
        Ok(())
    }
}

fn assert_graph_contribution(processor: &mut BlockProcessor, gain_db: f32) {
    let frames = processor.plan().format().block_frames;
    let driver_capture: Vec<f32> = [1.0, -1.0, 0.5, -0.5]
        .into_iter()
        .cycle()
        .take(frames as usize * 2)
        .collect();
    let mut graph_contribution = vec![0.0; driver_capture.len()];
    // Prime the finite DeviceBus snapshot; Driver already owns OS playback mixing.
    processor
        .process_block(frames, &[&driver_capture], &mut [&mut graph_contribution])
        .unwrap();
    for _ in 0..2 {
        processor
            .process_block(frames, &[&driver_capture], &mut [&mut graph_contribution])
            .unwrap();
        let gain = 10.0_f32.powf(gain_db / 20.0);
        for (mixed_capture, contribution) in driver_capture.iter().zip(&graph_contribution) {
            assert!((contribution - mixed_capture * gain).abs() < 0.000_001);
        }
        assert!(graph_contribution.iter().any(|sample| *sample != 0.0));
    }
}

#[test]
fn virtual_gain_survives_disk_restart_and_does_not_change_prepared_device_specs() {
    use std::sync::atomic::Ordering;
    let fixture = crate::test_support::Documents::new("source-gain");
    let path = fixture.join("graphs.json");
    let initial = default_duplex_document(1, None);
    let NodeConfig::VirtualEndpoint(config) = &initial.nodes[0].config else {
        unreachable!()
    };
    let backend = Arc::new(RecordingEndpoint::default());
    *backend.current.lock() = vec![config.spec.clone()];
    let mut engine = controller(initial)
        .with_test_documents(&fixture.files)
        .with_graph_catalog_path(path.clone());
    engine.endpoint_backend = backend.clone();
    for gain_db in [-6.0, -12.0] {
        let mut document = engine.graph();
        document.revision += 1;
        let NodeConfig::VirtualEndpoint(endpoint) = &mut document.nodes[0].config else {
            unreachable!()
        };
        endpoint.gain_db = gain_db;
        assert!(
            engine
                .apply_graph_request(document.into())
                .unwrap()
                .persistence_error
                .is_none()
        );
        let mut processor = crate::controller_test_support::settle(&engine);
        assert_graph_contribution(&mut processor, gain_db);
    }
    assert_eq!(backend.prepares.load(Ordering::Relaxed), 2);
    assert_eq!(backend.changed_specs.load(Ordering::Relaxed), 0);
    assert_eq!(backend.rate_changes.load(Ordering::Relaxed), 0);
    assert!(
        String::from_utf8(fixture.files.bytes(&path).unwrap())
            .unwrap()
            .contains("gainDb")
    );
    drop(engine);
    let mut restarted = controller(GraphDocument::new(1, vec![], vec![]));
    restarted.endpoint_backend = backend.clone();
    let restarted = restarted
        .with_test_documents(&fixture.files)
        .with_graph_catalog_path(path);
    let restored = restarted.graph();
    let NodeConfig::VirtualEndpoint(endpoint) = &restored.nodes[0].config else {
        unreachable!()
    };
    assert_eq!(endpoint.gain_db.to_bits(), (-12.0_f32).to_bits());
    assert_eq!(endpoint.spec, backend.current.lock()[0]);
    let mut processor = crate::controller_test_support::settle(&restarted);
    assert_graph_contribution(&mut processor, -12.0);
    assert_eq!(backend.prepares.load(Ordering::Relaxed), 3);
    assert_eq!(backend.changed_specs.load(Ordering::Relaxed), 0);
    assert_eq!(backend.rate_changes.load(Ordering::Relaxed), 0);
}
