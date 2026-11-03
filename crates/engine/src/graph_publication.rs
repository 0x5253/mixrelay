//! Stages native resources and publishes one complete DSP revision transaction.

use crate::{
    CompileContext, EngineGraphController, ErrorCode, GraphApplyResponse, GraphApplyState,
    GraphDocument, HostAudioBackendError, PROTOCOL_VERSION, PlaybackBindings,
    ProcessorConfigurationError, ProcessorSubmitError, ProtocolError, execution_control,
    graph_catalog, session_error,
};

impl EngineGraphController {
    pub(super) fn apply_graph_locked(
        &self,
        mut document: GraphDocument,
        mut catalog: Option<graph_catalog::Catalog>,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        let reuse = self.processor_controls.reuse_transaction();
        self.validate_recorder_channel_change(&document)?;
        if let Some(candidate) = &mut catalog {
            self.reconcile_catalog_devices(candidate)?;
            self.preserve_catalog_provider_state(candidate)?;
            document = candidate.active_document().clone();
        }
        let result = self.publish_graph_locked(document, catalog)?;
        reuse.commit();
        Ok(result)
    }

    pub(super) fn publish_graph_locked(
        &self,
        document: GraphDocument,
        catalog: Option<graph_catalog::Catalog>,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        let context = *self.context.read();
        self.publish_graph_context(document, catalog, context, true)
    }

    pub(super) fn publish_graph_context(
        &self,
        document: GraphDocument,
        catalog: Option<graph_catalog::Catalog>,
        context: CompileContext,
        persistence_required: bool,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        let revisions = self.exchange.revisions();
        if document.revision <= revisions.submitted {
            return Err(ProtocolError {
                code: ErrorCode::GraphRevisionStale,
                parameters: serde_json::json!({
                    "activeRevision": revisions.active,
                    "submittedRevision": revisions.submitted,
                    "proposedRevision": document.revision,
                }),
            });
        }
        let revision = document.revision;
        // Reject known durability failures before preparing native workers or streams.
        // The temporary document is still unpublished and drops on any later failure.
        let staged_catalog = match self.stage_catalog(catalog.as_ref()) {
            Ok(staged) => staged,
            Err(error) if !persistence_required => {
                self.catalog
                    .record_warning(Some(format!("engine stop persistence: {error:?}")));
                None
            }
            Err(error) => return Err(error),
        };
        let execution_control::PreparedExecution {
            processor,
            endpoints,
            document: runtime_document,
            view,
            playback_intents,
        } = self.prepare_execution(&document, catalog.as_ref(), context)?;
        let current = self.context.read().format;
        let resources = if current == context.format {
            None
        } else {
            Some(
                self.graph_resources
                    .prepare_format_change(
                        &runtime_document,
                        PlaybackBindings::from_plan(&playback_intents, &view.controls),
                        current,
                        context.format,
                    )
                    .map_err(|error| session_error(&error.to_string()))?,
            )
        };
        let meter_bindings = processor.plan().meter_bindings().to_vec();
        self.exchange
            .submit_configured(processor, || {
                self.endpoint_backend
                    .prepare_graph_attachments(&endpoints)?;
                let prepared = if let Some(resources) = resources {
                    self.runtime_status
                        .adopt_external_format(context.format)
                        .map(|()| resources.commit())
                } else {
                    self.graph_resources.reconcile(
                        &runtime_document,
                        PlaybackBindings::from_plan(&playback_intents, &view.controls),
                    )
                };
                if let Err(error) = prepared {
                    self.endpoint_backend.cancel_prepared_graph_attachments();
                    return Err(error);
                }
                self.stop_inactive_recorders(&document, catalog.as_ref());
                self.finish_catalog_write(staged_catalog);
                self.publish_execution_metadata(revision, meter_bindings, view);
                *self.context.write() = context;
                *self.graph.write() = document;
                self.replace_catalog_memory(catalog);
                Ok(())
            })
            .map_err(configuration_error)?;
        self.refresh_control_snapshot();
        self.catalog_restore.complete();
        Ok(GraphApplyResponse {
            protocol_version: PROTOCOL_VERSION,
            revision,
            state: GraphApplyState::Pending,
            persistence_error: self.graph_persistence_warning(),
        })
    }
}

fn exchange_error(error: ProcessorSubmitError) -> ProtocolError {
    match error {
        ProcessorSubmitError::Busy { proposed_revision } => ProtocolError {
            code: ErrorCode::GraphExchangeBusy,
            parameters: serde_json::json!({ "proposedRevision": proposed_revision }),
        },
        ProcessorSubmitError::WrongExchange { proposed_revision } => ProtocolError {
            code: ErrorCode::GraphInvalid,
            parameters: serde_json::json!({ "proposedRevision": proposed_revision, "reason": "runtime controls belong to another processor exchange" }),
        },
        ProcessorSubmitError::StaleRevision { current, proposed } => ProtocolError {
            code: ErrorCode::GraphRevisionStale,
            parameters: serde_json::json!({ "submittedRevision": current, "proposedRevision": proposed }),
        },
    }
}

fn configuration_error(error: ProcessorConfigurationError<HostAudioBackendError>) -> ProtocolError {
    match error {
        ProcessorConfigurationError::Busy { proposed_revision } => {
            exchange_error(ProcessorSubmitError::Busy { proposed_revision })
        }
        ProcessorConfigurationError::WrongExchange { proposed_revision } => {
            exchange_error(ProcessorSubmitError::WrongExchange { proposed_revision })
        }
        ProcessorConfigurationError::StaleRevision { current, proposed } => {
            exchange_error(ProcessorSubmitError::StaleRevision { current, proposed })
        }
        ProcessorConfigurationError::Configuration(HostAudioBackendError::PermissionPending {
            capability,
        }) => ProtocolError {
            code: ErrorCode::AudioPermissionPending,
            parameters: serde_json::json!({ "capability": capability, "configurationOutcome": "not_applied" }),
        },
        ProcessorConfigurationError::Configuration(HostAudioBackendError::PermissionDenied {
            capability,
        }) => ProtocolError {
            code: ErrorCode::AudioPermissionDenied,
            parameters: serde_json::json!({ "capability": capability, "configurationOutcome": "not_applied" }),
        },
        ProcessorConfigurationError::Configuration(error) => ProtocolError {
            code: ErrorCode::AudioDeviceDisconnected,
            parameters: serde_json::json!({ "reason": error.to_string() }),
        },
    }
}
