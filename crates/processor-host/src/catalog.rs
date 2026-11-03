//! Installed processor identities, saved port selection and graph node resolution.

use crate::{RackLocator, discovery, rack_formats};
use mixrelay_audio_graph::{
    BuiltInNodeResolver, CompileContext, CompiledPort, NodeConfig, NodeResolutionError,
    NodeResolver, NodeSpec, PortDirection, ProviderNodeConfig, ResolvedNode,
};
use mixrelay_control_protocol::{
    ExternalProcessorCatalog, ExternalProcessorDescriptor, PROTOCOL_VERSION,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

#[derive(Debug, Clone)]
pub(super) struct CatalogEntry {
    pub(super) descriptor: ExternalProcessorDescriptor,
    pub(super) rack: RackCatalogTarget,
}

#[derive(Debug, Clone)]
pub(super) struct RackCatalogTarget {
    pub(super) locator: RackLocator,
}

pub(super) fn selected_ports(
    descriptor: &ExternalProcessorDescriptor,
    config: &ProviderNodeConfig,
) -> Result<mixrelay_audio_graph::ProviderPorts, String> {
    let selected_id = config
        .ports
        .as_ref()
        .map_or(descriptor.default_layout_id.as_str(), |ports| {
            ports.layout_id.as_str()
        });
    let actual = descriptor
        .layouts
        .iter()
        .find(|ports| ports.layout_id == selected_id)
        .ok_or("selected processor layout is not installed")?;
    actual.validate().map_err(|error| error.to_string())?;
    if let Some(authored) = &config.ports {
        authored.validate().map_err(|error| error.to_string())?;
        if !rack_formats::layouts::same_topology(actual, authored) {
            return Err("saved processor topology differs from the native catalog".into());
        }
    }
    Ok(actual.clone())
}

#[derive(Debug, Clone, Default)]
pub struct ProcessorCatalog {
    pub(super) snapshot: Arc<RwLock<discovery::CatalogSnapshot>>,
    /// Discovery does not block catalog readers or the render thread; only rescans serialize.
    pub(super) scan_lock: Arc<Mutex<()>>,
}

impl ProcessorCatalog {
    /// Returns the installed port contract without creating a worker.
    #[must_use]
    pub fn ports(&self, type_id: &str) -> Option<mixrelay_audio_graph::ProviderPorts> {
        let entry = self.entry(type_id)?;
        entry
            .descriptor
            .layouts
            .iter()
            .find(|ports| ports.layout_id == entry.descriptor.default_layout_id)
            .cloned()
    }
    /// Safely discovers native and cross-platform effects. Each candidate is probed in a
    /// disposable copy of the engine executable so a broken plug-in cannot terminate the graph
    /// service.
    #[must_use]
    pub fn discover() -> Self {
        let catalog = Self::default();
        catalog.rescan(&[]);
        catalog
    }

    /// Refreshes the discovery snapshot using standard format locations plus explicit user paths.
    /// Scanner and probe processes isolate malformed third-party bundles from the engine service.
    /// Failed formats/candidates retain their previous entries until a successful retry.
    pub fn rescan(&self, custom_paths: &[PathBuf]) {
        discovery::rescan(self, custom_paths);
    }

    #[must_use]
    pub fn protocol_catalog(&self) -> ExternalProcessorCatalog {
        let snapshot = self
            .snapshot
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ExternalProcessorCatalog {
            protocol_version: PROTOCOL_VERSION,
            processors: snapshot
                .entries
                .values()
                .map(|entry| entry.descriptor.clone())
                .collect(),
            diagnostics: snapshot.diagnostics.clone(),
        }
    }

    pub(super) fn entry(&self, type_id: &str) -> Option<CatalogEntry> {
        self.snapshot
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .get(type_id)
            .cloned()
    }

    #[must_use]
    pub fn parameter_is_writable(&self, type_id: &str, parameter_id: u32) -> bool {
        self.entry(type_id).is_some_and(|entry| {
            entry
                .descriptor
                .parameters
                .iter()
                .any(|parameter| parameter.id == parameter_id && !parameter.is_read_only)
        })
    }
}

#[derive(Debug, Clone)]
pub struct CatalogNodeResolver {
    catalog: ProcessorCatalog,
}

impl CatalogNodeResolver {
    #[must_use]
    pub const fn new(catalog: ProcessorCatalog) -> Self {
        Self { catalog }
    }
}

impl NodeResolver for CatalogNodeResolver {
    fn resolve(
        &self,
        node: &NodeSpec,
        context: &CompileContext,
    ) -> Result<ResolvedNode, NodeResolutionError> {
        let NodeConfig::Provider(config) = &node.config else {
            return BuiltInNodeResolver.resolve(node, context);
        };
        let ports =
            match self.catalog.entry(config.type_id.as_str()) {
                Some(entry) => selected_ports(&entry.descriptor, config)
                    .map_err(|reason| NodeResolutionError::InvalidConfiguration { reason })?,
                None => config.ports.clone().ok_or_else(|| {
                    NodeResolutionError::CapabilityUnavailable {
                        reason: format!("processor is not installed: {}", config.type_id),
                    }
                })?,
            };
        ports
            .validate()
            .map_err(|error| NodeResolutionError::InvalidConfiguration {
                reason: error.to_string(),
            })?;
        if ports
            .inputs
            .iter()
            .chain(&ports.outputs)
            .any(|bus| bus.channels > context.limits.max_channels_per_port)
        {
            return Err(NodeResolutionError::InvalidConfiguration {
                reason: "processor bus width exceeds graph limits".to_owned(),
            });
        }
        Ok(ResolvedNode {
            inputs: ports
                .inputs
                .iter()
                .map(|bus| CompiledPort::new(bus.id.clone(), PortDirection::Input, bus.channels))
                .collect(),
            outputs: ports
                .outputs
                .iter()
                .map(|bus| CompiledPort::new(bus.id.clone(), PortDirection::Output, bus.channels))
                .collect(),
        })
    }
}
