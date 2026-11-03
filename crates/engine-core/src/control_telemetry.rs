//! Batched committed state observations, independent of native mutation preparation.
use crate::{EngineGraphController, NodeConfig, processor_editor_error};
use mixrelay_control_protocol::{
    AppStatus, ControlStateSnapshot, GraphRuntimeStatus, ProcessorEditorStatus, ProtocolError,
};
use mixrelay_time::Instant;
use std::time::Duration;

pub(super) struct CachedStatus {
    observed: Instant,
    graph: GraphRuntimeStatus,
    enabled: bool,
    status: AppStatus,
}

impl EngineGraphController {
    pub(super) fn observe_control_state(&self) -> Option<ControlStateSnapshot> {
        // Read committed controls without waiting for a native writer.
        let mutation = self.mutation.try_lock()?;
        let graph = self.graph_status();
        let editors = self
            .graph()
            .nodes
            .iter()
            .filter(|node| matches!(node.config, NodeConfig::Provider(_)))
            .filter_map(|node| self.processor_editor_status_locked(node.id.as_str()).ok())
            .collect();
        let controls = self.snapshot_controls().ok();
        drop(mutation);
        // Driver inventory can wait on IPC. It must not own writer admission.
        let status = self.telemetry_status(graph);
        let _mutation = self.mutation.try_lock()?;
        if self.graph_status() != graph {
            return None;
        }
        Some(ControlStateSnapshot {
            generation: 0,
            status,
            graph,
            controls,
            editors,
        })
    }

    fn telemetry_status(&self, graph: GraphRuntimeStatus) -> AppStatus {
        let mut cached = self.telemetry_status.lock();
        let enabled = self.engine_power_status().enabled;
        // Native Driver inventory remains a one-second observation, rather than twenty
        // socket round trips per second. Revision changes force immediate refresh.
        let fresh = cached.as_ref().filter(|old| {
            self.clock.now().duration_since(old.observed) < Duration::from_secs(1)
                && old.graph == graph
                && old.enabled == enabled
        });
        let mut status = if let Some(fresh) = fresh {
            fresh.status.clone()
        } else {
            let status = self.status();
            *cached = Some(CachedStatus {
                observed: self.clock.now(),
                graph,
                enabled,
                status: status.clone(),
            });
            status
        };
        status.engine_power = Some(self.engine_power_status());
        status.processor_timings = self.processor_timings();
        status.processor_latencies = self.processor_latencies();
        status.failed_processors = self.failed_processor_status();
        status.recorder_error = self.recorders.cleanup_error();
        status
    }

    /// The caller holds mutation admission for the selected graph identity.
    pub(super) fn processor_editor_status_locked(
        &self,
        node_id: &str,
    ) -> Result<ProcessorEditorStatus, ProtocolError> {
        let runtime_id = self.external_processor_node(node_id)?;
        let mut status = self
            .processor_controls
            .editor_status(&runtime_id)
            .map_err(|reason| processor_editor_error(node_id, &reason.to_string()))?;
        node_id.clone_into(&mut status.node_id);
        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GraphDocument;

    #[test]
    fn graph_status_exposes_permission_wait_without_taking_the_preparation_lock() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        struct PermissionStatus(AtomicBool);
        impl crate::RuntimeStatusBackend for PermissionStatus {
            fn status(&self, _: mixrelay_audio_graph::RenderFormat) -> AppStatus {
                AppStatus::default()
            }
            fn awaiting_microphone_permission(&self) -> bool {
                self.0.load(Ordering::Relaxed)
            }
        }

        impl crate::ProcessingControlBackend for PermissionStatus {}

        impl crate::SessionFormatBackend for PermissionStatus {}
        let backend = Arc::new(PermissionStatus(AtomicBool::new(false)));
        let mut controller =
            crate::endpoint_settings_tests::controller(GraphDocument::new(7, vec![], vec![]));
        controller.runtime_status = backend.clone();
        let _preparation = controller.mutation.lock();
        assert_eq!(controller.graph_status().pending_reason, None);
        backend.0.store(true, Ordering::Relaxed);
        let waiting = controller.graph_status();
        assert_eq!(
            waiting.pending_reason,
            Some(mixrelay_control_protocol::GraphPendingReason::AwaitingMicrophonePermission)
        );
        assert_eq!(
            serde_json::to_value(waiting).unwrap()["pendingReason"],
            "awaiting_microphone_permission"
        );
        backend.0.store(false, Ordering::Relaxed);
        assert_eq!(controller.graph_status().pending_reason, None);
    }

    struct DriverReadback {
        owner: parking_lot::Mutex<std::sync::Weak<EngineGraphController>>,
        change_revision: bool,
        observed: std::sync::atomic::AtomicBool,
    }

    impl crate::RuntimeStatusBackend for DriverReadback {
        fn status(&self, _: mixrelay_audio_graph::RenderFormat) -> AppStatus {
            if self
                .observed
                .swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                return AppStatus::default();
            }
            let owner = self.owner.lock().upgrade().unwrap();
            assert!(
                owner.mutation.try_lock().is_some(),
                "read-only Driver IPC must not make recorder commands busy"
            );
            if self.change_revision {
                let revision = owner.exchange.revisions().submitted + 1;
                owner
                    .apply_graph_request(GraphDocument::new(revision, vec![], vec![]).into())
                    .unwrap();
            }
            AppStatus::default()
        }
    }

    impl crate::ProcessingControlBackend for DriverReadback {}

    impl crate::SessionFormatBackend for DriverReadback {}

    #[test]
    fn telemetry_driver_readback_releases_writer_admission_without_a_ui_client() {
        for change_revision in [false, true] {
            let backend = std::sync::Arc::new(DriverReadback {
                owner: parking_lot::Mutex::new(std::sync::Weak::new()),
                change_revision,
                observed: std::sync::atomic::AtomicBool::new(false),
            });
            let mut controller =
                crate::endpoint_settings_tests::controller(GraphDocument::new(7, vec![], vec![]));
            controller.runtime_status = backend.clone();
            let controller = std::sync::Arc::new(controller);
            *backend.owner.lock() = std::sync::Arc::downgrade(&controller);
            let state = controller.telemetry_state();
            if change_revision {
                assert!(
                    state.is_none(),
                    "discard a snapshot invalidated during Driver IPC"
                );
                assert_eq!(controller.graph().revision, 8);
            } else {
                let state = state.unwrap();
                assert_eq!(state.graph.submitted_revision, 7);
                assert_eq!(state.controls.unwrap().revision, 7);
            }
            assert!(controller.try_node_control().is_ok());
        }
    }

    #[test]
    fn telemetry_defers_native_writers_and_caches_inventory_until_expiry_or_power_change() {
        let mut controller =
            crate::endpoint_settings_tests::controller(GraphDocument::new(7, vec![], vec![]));
        let clock = std::sync::Arc::new(mixrelay_time::ManualClock::default());
        controller.clock = clock.clone();
        let writer = controller.mutation.lock();
        assert!(controller.telemetry_state().is_none());
        assert_eq!(controller.meters().protocol_version, 1);
        drop(writer);
        let state = controller.telemetry_state().unwrap();
        assert_eq!(state.graph.submitted_revision, 7);
        assert_eq!(state.controls.unwrap().revision, 7);
        let first = controller
            .telemetry_status
            .lock()
            .as_ref()
            .unwrap()
            .observed;
        assert!(controller.telemetry_state().is_some());
        assert_eq!(
            controller
                .telemetry_status
                .lock()
                .as_ref()
                .unwrap()
                .observed,
            first
        );
        clock.advance(Duration::from_secs(2));
        assert!(controller.telemetry_state().is_some());
        assert!(
            controller
                .telemetry_status
                .lock()
                .as_ref()
                .unwrap()
                .observed
                >= first
        );
        controller.power.set_enabled(false);
        let state = controller.telemetry_state().unwrap();
        assert!(!state.status.engine_power.unwrap().enabled);
        assert!(!controller.telemetry_status.lock().as_ref().unwrap().enabled);
    }
}
