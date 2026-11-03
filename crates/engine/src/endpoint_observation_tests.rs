use super::{Catalog, EngineGraphController};
use crate::endpoint_settings_tests::{controller, endpoint_document};
use mixrelay_audio_graph::{GraphDocument, NodeConfig, VirtualEndpointId};
use mixrelay_audio_io::{
    HostAudioBackendError, VirtualEndpointBackend, VirtualEndpointCapabilities,
};
use parking_lot::Mutex;
use std::{collections::BTreeMap, sync::Arc};

struct Observations {
    rates: BTreeMap<VirtualEndpointId, u32>,
    reads: Mutex<Vec<VirtualEndpointId>>,
    capabilities: VirtualEndpointCapabilities,
}

impl VirtualEndpointBackend for Observations {
    fn capabilities(&self) -> &VirtualEndpointCapabilities {
        &self.capabilities
    }

    fn prepare_graph_attachments(
        &self,
        _: &[mixrelay_audio_graph::VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError> {
        panic!("metadata observation must not prepare native resources")
    }

    fn sample_rate(&self, id: &VirtualEndpointId) -> Result<u32, HostAudioBackendError> {
        self.reads.lock().push(id.clone());
        self.rates
            .get(id)
            .copied()
            .ok_or_else(|| HostAudioBackendError::BackendFailure {
                message: "device disconnected".into(),
            })
    }

    fn set_sample_rate(&self, _: &VirtualEndpointId, _: u32) -> Result<(), HostAudioBackendError> {
        panic!("observing a DAW change must never call the native setter")
    }
}

fn fixture(rate: Option<u32>) -> (EngineGraphController, Arc<Observations>, GraphDocument) {
    let document = endpoint_document(7, "Shared device");
    let NodeConfig::VirtualEndpoint(config) = &document.nodes[0].config else {
        panic!()
    };
    let mut engine = controller(document.clone());
    let backend = Arc::new(Observations {
        rates: rate
            .map(|rate| (config.id.clone(), rate))
            .into_iter()
            .collect(),
        reads: Mutex::new(Vec::new()),
        capabilities: engine.endpoint_backend.capabilities().clone(),
    });
    engine.endpoint_backend = backend.clone();
    (engine, backend, document)
}

#[test]
fn observes_devices_outside_the_selected_graph_without_modifying_the_view() {
    let (engine, backend, document) = fixture(Some(96_000));
    let mut catalog = Some(Catalog::imported(document.clone()));
    let mut selected = GraphDocument::new(8, vec![], vec![]);
    let unchanged = selected.clone();
    assert!(
        engine
            .observe_endpoint_catalog(&mut selected, &mut catalog)
            .unwrap()
    );
    assert_eq!(selected, unchanged);
    let mut expected = document;
    let NodeConfig::VirtualEndpoint(config) = &mut expected.nodes[0].config else {
        panic!()
    };
    config.default_sample_rate = 96_000;
    assert_eq!(catalog.unwrap().active_document(), &expected);
    assert_eq!(backend.reads.lock().len(), 1);
}

#[test]
fn observes_shared_identity_once_and_updates_both_references() {
    let (engine, backend, mut selected) = fixture(Some(96_000));
    let mut second = selected.nodes[0].clone();
    second.id = "second-reference".into();
    selected.nodes.push(second);
    let mut catalog = Some(Catalog::imported(selected.clone()));
    assert!(
        engine
            .observe_endpoint_catalog(&mut selected, &mut catalog)
            .unwrap()
    );
    assert_eq!(catalog.as_ref().unwrap().active_document(), &selected);
    for node in &selected.nodes {
        let NodeConfig::VirtualEndpoint(config) = &node.config else {
            panic!("fixture contains virtual references only");
        };
        assert_eq!(config.default_sample_rate, 96_000);
    }
    assert_eq!(backend.reads.lock().len(), 1);
    assert!(
        !engine
            .observe_endpoint_catalog(&mut selected, &mut catalog)
            .unwrap()
    );
}

#[test]
fn missing_or_invalid_native_rates_do_not_mutate_candidates() {
    for rate in [None, Some(0), Some(12345)] {
        let (engine, _, mut selected) = fixture(rate);
        let original = selected.clone();
        let mut catalog = Some(Catalog::imported(selected.clone()));
        let result = engine.observe_endpoint_catalog(&mut selected, &mut catalog);
        if rate.is_none() {
            assert!(!result.unwrap());
        } else {
            assert!(result.is_err());
        }
        assert_eq!(selected, original);
        assert_eq!(catalog.unwrap().active_document(), &original);
    }
}
