//! Driver-owned device identity. Graphs carry reference snapshots, not device intent.
use std::collections::{BTreeMap, BTreeSet};

use mixrelay_audio_graph::{GraphDocument, NodeConfig, VirtualEndpointId, VirtualEndpointSpec};
use mixrelay_audio_io::VirtualEndpointSettings;
use mixrelay_control_protocol::ProtocolError;

use super::{Catalog, EngineGraphController, invalid};

#[cfg(test)]
#[path = "endpoint_observation_tests.rs"]
mod observation_tests;

fn specs(
    document: &GraphDocument,
) -> Result<BTreeMap<VirtualEndpointId, VirtualEndpointSpec>, ProtocolError> {
    let mut devices = BTreeMap::new();
    for node in &document.nodes {
        if let NodeConfig::VirtualEndpoint(config) = &node.config {
            config
                .spec
                .validate_contract(64, 64)
                .map_err(|error| invalid(&error.to_string()))?;
            devices.insert(config.id.clone(), config.spec.clone());
        }
    }
    Ok(devices)
}

impl Catalog {
    /// A view cannot create a second native owner by copying a device snapshot.
    pub(super) fn device_specs(
        &self,
    ) -> Result<BTreeMap<VirtualEndpointId, VirtualEndpointSpec>, ProtocolError> {
        let mut devices = BTreeMap::new();
        for (_, document) in self.documents() {
            devices.extend(specs(document)?);
        }
        Ok(devices)
    }
}

impl EngineGraphController {
    /// Replaces copied graph device metadata only in the unpublished execution input.
    /// Driver mutations are explicit API operations, never inferred from graph edits.
    /// # Errors
    /// Rejects missing Driver identities or unavailable authoritative settings.
    pub(crate) fn resolve_execution_devices(
        &self,
        documents: &mut [(String, GraphDocument)],
    ) -> Result<(), ProtocolError> {
        let desired: BTreeMap<_, _> = documents
            .iter()
            .flat_map(|(_, graph)| &graph.nodes)
            .filter_map(|node| match &node.config {
                NodeConfig::VirtualEndpoint(config) => {
                    Some((config.id.clone(), config.spec.clone()))
                }
                _ => None,
            })
            .collect();
        if desired.is_empty() {
            return Ok(());
        }
        let actual: BTreeMap<_, _> = self
            .endpoint_backend
            .resolve_graph_devices(&desired.into_values().collect::<Vec<_>>())
            .map_err(|error| invalid(&error.to_string()))?
            .into_iter()
            .map(|spec| (spec.id.clone(), spec))
            .collect();
        for (_, graph) in documents {
            for node in &mut graph.nodes {
                if let NodeConfig::VirtualEndpoint(config) = &mut node.config {
                    config.spec.clone_from(
                        actual
                            .get(&config.id)
                            .ok_or_else(|| invalid("Driver device is missing"))?,
                    );
                }
            }
        }
        Ok(())
    }
    /// Builds a single observed-rate snapshot for session publication or startup recall.
    pub(crate) fn observed_catalog_candidate(
        &self,
        document: &mut GraphDocument,
    ) -> Result<Option<Catalog>, ProtocolError> {
        let mut catalog = self.catalog_candidate(document);
        self.observe_endpoint_catalog(document, &mut catalog)?;
        if let Some(candidate) = &mut catalog {
            candidate.update_document(document.clone());
        }
        Ok(catalog)
    }

    /// Reads each catalog-owned native clock once, then updates only rate metadata
    /// in the caller's unpublished candidate. The caller holds the mutation lock;
    /// publication and persistence remain the normal graph transaction's responsibility.
    ///
    /// # Errors
    /// Rejects invalid native rates without changing either candidate. An unavailable
    /// readback leaves that device's metadata unchanged, never inventing a clock.
    pub(crate) fn observe_endpoint_catalog(
        &self,
        document: &mut GraphDocument,
        catalog: &mut Option<Catalog>,
    ) -> Result<bool, ProtocolError> {
        let mut identities = BTreeSet::new();
        collect_identities(document, &mut identities);
        if let Some(candidate) = catalog.as_ref() {
            for (_, graph) in candidate.documents() {
                collect_identities(graph, &mut identities);
            }
        }
        let mut rates = BTreeMap::new();
        for id in identities {
            let Ok(rate) = self.endpoint_backend.sample_rate(&id) else {
                continue;
            };
            if rate == 0
                || !self
                    .endpoint_backend
                    .capabilities()
                    .supported_sample_rates
                    .contains(&rate)
            {
                return Err(invalid("native device reported an unsupported sample rate"));
            }
            rates.insert(id, rate);
        }
        let mut changed = apply_observed_rates(document, &rates);
        if let Some(candidate) = catalog.as_mut() {
            for (_, graph) in candidate.documents_mut() {
                changed |= apply_observed_rates(graph, &rates);
            }
        }
        Ok(changed)
    }

    pub(crate) fn endpoint_settings(&self) -> Option<VirtualEndpointSettings> {
        self.endpoint_backend.settings()
    }

    /// Refreshes all catalog references from the Driver before publication.
    /// # Errors
    /// Rejects missing devices and invalid observed specifications without Driver mutation.
    pub(crate) fn reconcile_catalog_devices(
        &self,
        candidate: &mut Catalog,
    ) -> Result<(), ProtocolError> {
        let mut documents: Vec<_> = candidate
            .documents()
            .map(|(id, graph)| (id.to_owned(), graph.clone()))
            .collect();
        self.resolve_execution_devices(&mut documents)?;
        for ((_, graph), (_, resolved)) in candidate.documents_mut().zip(documents) {
            for (node, actual) in graph.nodes.iter_mut().zip(resolved.nodes) {
                if let (NodeConfig::VirtualEndpoint(config), NodeConfig::VirtualEndpoint(actual)) =
                    (&mut node.config, actual.config)
                {
                    config.spec = actual.spec;
                }
            }
        }
        candidate.device_specs()?;
        Ok(())
    }
}

fn collect_identities(document: &GraphDocument, identities: &mut BTreeSet<VirtualEndpointId>) {
    for node in &document.nodes {
        if let NodeConfig::VirtualEndpoint(config) = &node.config {
            identities.insert(config.id.clone());
        }
    }
}

fn apply_observed_rates(
    document: &mut GraphDocument,
    rates: &BTreeMap<VirtualEndpointId, u32>,
) -> bool {
    let mut changed = false;
    for node in &mut document.nodes {
        if let NodeConfig::VirtualEndpoint(config) = &mut node.config
            && let Some(&rate) = rates.get(&config.id)
        {
            changed |= config.default_sample_rate != rate;
            config.default_sample_rate = rate;
            if !config.supported_sample_rates.contains(&rate) {
                config.supported_sample_rates.push(rate);
                config.supported_sample_rates.sort_unstable();
                changed = true;
            }
        }
    }
    changed
}
