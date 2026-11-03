use super::{Catalog, EngineGraphController, ProtocolError, unavailable};
use mixrelay_audio_graph::{NodeConfig, VirtualEndpointId};

struct RateChange {
    id: VirtualEndpointId,
    previous: u32,
    desired: u32,
}

impl EngineGraphController {
    pub(super) fn publish_workspace_rates(&self, candidate: Catalog) -> Result<(), ProtocolError> {
        let changes = self.workspace_rate_changes(&candidate)?;
        if changes.is_empty() {
            return self
                .publish_graph_locked(candidate.active_document().clone(), Some(candidate))
                .map(|_| ());
        }
        // Validate the entire topology before touching native clocks. Only this detached
        // preflight copy uses the existing rates; the accepted document keeps file settings.
        let mut preflight = candidate.clone();
        for (_, graph) in preflight.documents_mut() {
            for node in &mut graph.nodes {
                if let NodeConfig::VirtualEndpoint(endpoint) = &mut node.config
                    && let Some(change) = changes.iter().find(|change| change.id == endpoint.id)
                {
                    endpoint.spec.default_sample_rate = change.previous;
                    if !endpoint
                        .spec
                        .supported_sample_rates
                        .contains(&change.previous)
                    {
                        endpoint.spec.supported_sample_rates.push(change.previous);
                    }
                }
            }
        }
        self.compose_execution(
            preflight.active_document(),
            Some(&preflight),
            *self.context.read(),
        )?;
        if let Err(error) = self.runtime_status.suspend() {
            self.runtime_status.resume();
            return Err(unavailable(&error.to_string()));
        }
        let mut attempted = 0;
        let result = self.negotiate_workspace_rates(candidate, &changes, &mut attempted);
        if result.is_ok() || self.restore_workspace_rates(&changes[..attempted]).is_ok() {
            self.runtime_status.resume();
        } else {
            self.runtime_status.fail_reconfiguration();
            return Err(unavailable(
                "workspace change failed and native clock rollback failed",
            ));
        }
        result
    }

    fn workspace_rate_changes(
        &self,
        candidate: &Catalog,
    ) -> Result<Vec<RateChange>, ProtocolError> {
        for (_, document) in candidate.documents() {
            crate::virtual_endpoint_specs(document, self.endpoint_backend.capabilities())?;
        }
        Ok(candidate
            .device_specs()?
            .into_iter()
            .filter_map(|(id, spec)| {
                let previous = self.endpoint_backend.sample_rate(&id).ok()?;
                (previous != spec.default_sample_rate).then_some(RateChange {
                    id,
                    previous,
                    desired: spec.default_sample_rate,
                })
            })
            .collect())
    }

    fn negotiate_workspace_rates(
        &self,
        candidate: Catalog,
        changes: &[RateChange],
        attempted: &mut usize,
    ) -> Result<(), ProtocolError> {
        for change in changes {
            if self.endpoint_backend.sample_rate(&change.id).ok() != Some(change.previous) {
                return Err(unavailable(
                    "device clock changed externally before workspace negotiation",
                ));
            }
            *attempted += 1;
            self.write_endpoint_rate(&change.id, change.previous, change.desired)?;
        }
        let mut context = *self.context.read();
        if let Some(observed) = self.runtime_status.external_format(context.format) {
            context.format = observed;
        }
        self.publish_graph_context(
            candidate.active_document().clone(),
            Some(candidate),
            context,
            true,
        )
        .map(|_| ())
    }

    fn restore_workspace_rates(&self, changes: &[RateChange]) -> Result<(), ProtocolError> {
        for change in changes.iter().rev() {
            let observed = self
                .endpoint_backend
                .sample_rate(&change.id)
                .map_err(|error| unavailable(&error.to_string()))?;
            if observed != change.previous && observed != change.desired {
                return Err(unavailable(
                    "device clock changed externally; cannot roll back",
                ));
            }
            self.write_endpoint_rate(&change.id, observed, change.previous)?;
        }
        Ok(())
    }
}
