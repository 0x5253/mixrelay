//! Prepares isolated processors or adopts an exact surviving instance for a render plan.

use crate::{
    ProcessorCatalog, ProcessorControlRegistry, catalog::selected_ports,
    preparation::WorkerPreparation, reuse,
};
use mixrelay_render::{
    HostedAudioProcessor, HostedProcessorFactory, HostedProcessorPrepareRequest,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct ExternalProcessorFactory {
    catalog: ProcessorCatalog,
    controls: ProcessorControlRegistry,
    preparation: Arc<dyn WorkerPreparation>,
}

struct UnavailableProcessor;

impl HostedAudioProcessor for UnavailableProcessor {
    fn process(&mut self, _input: &[f32], output: &mut [f32], _frames: usize) -> bool {
        output.fill(0.0);
        true
    }
}

impl ExternalProcessorFactory {
    pub(super) fn with_preparation(
        catalog: ProcessorCatalog,
        controls: ProcessorControlRegistry,
        preparation: Arc<dyn WorkerPreparation>,
    ) -> Self {
        Self {
            catalog,
            controls,
            preparation,
        }
    }
    fn prepare_worker(
        &self,
        entry: &crate::catalog::CatalogEntry,
        config: &mixrelay_audio_graph::ProviderNodeConfig,
        format: mixrelay_audio_graph::RenderFormat,
    ) -> Result<crate::preparation::PreparedWorker, crate::preparation::WorkerPreparationError>
    {
        let instance_id = self.controls.instances.issue()?;
        let prepared = self
            .preparation
            .prepare(entry, config, format, instance_id)?;
        let observed = prepared.control.shared.header().instance_id;
        if observed != instance_id.get() {
            return Err(
                crate::preparation::WorkerPreparationError::InstanceMismatch {
                    expected: instance_id,
                    observed,
                },
            );
        }
        Ok(prepared)
    }
}

impl HostedProcessorFactory for ExternalProcessorFactory {
    fn prepare_inactive(
        &self,
        request: HostedProcessorPrepareRequest<'_>,
    ) -> Result<Option<Box<dyn HostedAudioProcessor>>, String> {
        self.controls
            .verify_state(&request.config.parameters)
            .map_err(|error| error.to_string())?;
        let mut config = request.config.clone();
        if let Some(entry) = self.catalog.entry(config.type_id.as_str()) {
            config.ports = Some(selected_ports(&entry.descriptor, &config)?);
        }
        let normalized = HostedProcessorPrepareRequest {
            config: &config,
            ..request
        };
        Ok(self.controls.reuse_request(&normalized))
    }

    fn prepare(
        &self,
        request: HostedProcessorPrepareRequest<'_>,
    ) -> Result<Box<dyn HostedAudioProcessor>, String> {
        self.controls
            .verify_state(&request.config.parameters)
            .map_err(|error| error.to_string())?;
        let Some(entry) = self.catalog.entry(request.config.type_id.as_str()) else {
            if !request.config.ports.as_ref().is_some_and(|ports| {
                ports.total_input_channels() == Ok(request.input_channels)
                    && ports.total_output_channels() == Ok(request.output_channels)
            }) {
                return Err("unavailable processor needs its saved port contract".into());
            }
            return Ok(Box::new(UnavailableProcessor));
        };
        let selected = selected_ports(&entry.descriptor, request.config)?;
        if selected
            .total_input_channels()
            .map_err(|error| error.to_string())?
            != request.input_channels
            || selected
                .total_output_channels()
                .map_err(|error| error.to_string())?
                != request.output_channels
        {
            return Err("compiled plug-in layout does not match the catalog".to_owned());
        }
        // Reuse is tied to the resolved topology even when the authored node asks for
        // the current default. A catalog change must never inherit a same-width worker
        // whose sidechain ordering or native configuration identity has changed.
        let mut config = request.config.clone();
        config.ports = Some(selected);
        let request = HostedProcessorPrepareRequest {
            config: &config,
            ..request
        };
        if let Some(processor) = self.controls.reuse_request(&request) {
            return Ok(processor);
        }
        let prepared = self
            .prepare_worker(&entry, request.config, request.format)
            .map_err(|error| error.to_string())?;
        *prepared
            .control
            .reuse
            .lock()
            .map_err(|_| "processor reuse metadata is poisoned")? =
            Some(reuse::ReuseSignature::new(&request));
        let registration = self.controls.register(request.node_id, &prepared.control);
        Ok(prepared.processor.register(registration))
    }
}

#[cfg(test)]
mod tests;
