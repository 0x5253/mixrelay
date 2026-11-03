use super::*;
use crate::shared_transport::WorkerControl;
use crate::worker::broker::publish_editor_failure;
use crate::worker::broker::publish_observed_editor_snapshot;
use crate::*;
use mixrelay_audio_graph::{ProviderNodeConfig, RenderFormat};
use mixrelay_control_protocol::ProcessorEditorState;
use mixrelay_render::HostedProcessorPrepareRequest;
use std::sync::{Arc, atomic::Ordering};

struct Fixture {
    control: Arc<HostedProcessorControl>,
    worker: Arc<WorkerControl>,
    capture: crate::state_capture::memory::MemoryCapture,
}

impl Fixture {
    fn new(instance: u64) -> Self {
        let (_, mailbox, _, worker) = shared_transport::memory::endpoints(
            AudioLayout {
                inputs: 2,
                outputs: 2,
                block_frames: 4,
            },
            crate::instance_id::InstanceId::new(instance).unwrap(),
        );
        let capture = crate::state_capture::memory::MemoryCapture::default();
        let control = Arc::new(HostedProcessorControl::new(
            Arc::new(mailbox),
            Arc::new(mixrelay_time::ManualClock::default()),
            Box::new(capture.clone()),
        ));
        worker
            .header()
            .worker_status
            .store(WORKER_READY, Ordering::Release);
        Self {
            control,
            worker: Arc::new(worker),
            capture,
        }
    }

    fn control(&self) -> &Arc<HostedProcessorControl> {
        &self.control
    }

    fn observed(&self, open: bool) {
        publish_observed_editor_snapshot(
            self.worker.header(),
            rack_formats::EditorSnapshot {
                open,
                ..rack_formats::EditorSnapshot::default()
            },
        );
    }
}

#[test]
fn failed_open_survives_idle_ticks_until_an_explicit_editor_command() {
    let fixture = Fixture::new(1);
    let control = fixture.control();
    let header = fixture.worker.header();
    let node = NodeId::from("effect");
    for failure in [EDITOR_FAILURE_PLUGIN_REJECTED, EDITOR_FAILURE_HOST_WINDOW] {
        publish_editor_failure(header, failure);
        for _ in 0..20 {
            fixture.observed(false);
            assert_eq!(
                control.editor_status(&node).state,
                ProcessorEditorState::Failed
            );
            assert!(control.editor_status(&node).failure.is_some());
        }
        control.set_editor_open(true);
        fixture.observed(true);
        assert_eq!(
            control.editor_status(&node).state,
            ProcessorEditorState::Open
        );
        assert!(control.editor_status(&node).failure.is_none());
        publish_editor_failure(header, failure);
        control.set_editor_open(false);
        fixture.observed(false);
        assert_eq!(
            control.editor_status(&node).state,
            ProcessorEditorState::Closed
        );
        assert!(control.editor_status(&node).failure.is_none());
    }
}

#[test]
fn requested_close_is_not_an_observation_and_native_close_is_not_consumed_by_readers() {
    let fixture = Fixture::new(1);
    let registry = ProcessorControlRegistry::default();
    let node = NodeId::from("effect");
    let _registration = registry.register(&node, fixture.control());
    assert_eq!(
        registry
            .editor_close_observation(&node)
            .unwrap()
            .close_generation,
        0
    );
    fixture.observed(false);
    fixture.observed(true);
    registry.set_editor_open(&node, false).unwrap();
    assert_eq!(
        registry.editor_status(&node).unwrap().state,
        ProcessorEditorState::Open
    );
    assert_eq!(
        registry
            .editor_close_observation(&node)
            .unwrap()
            .close_generation,
        0
    );
    fixture.observed(false);
    let closed = registry.editor_close_observation(&node).unwrap();
    assert_eq!(closed.close_generation, 1);
    assert_eq!(registry.editor_close_observation(&node).unwrap(), closed);
    fixture.observed(false);
    assert_eq!(registry.editor_close_observation(&node).unwrap(), closed);
    fixture.observed(true);
    fixture.observed(false);
    assert_eq!(
        registry
            .editor_close_observation(&node)
            .unwrap()
            .close_generation,
        2
    );
    fixture
        .worker
        .header()
        .worker_status
        .store(WORKER_FAILED, Ordering::Release);
    assert!(registry.editor_close_observation(&node).is_err());
    assert_eq!(
        registry.editor_status(&node).unwrap().state,
        ProcessorEditorState::Failed
    );
}

#[test]
fn stale_close_tokens_cannot_snapshot_replaced_workers_or_newer_events() {
    let first = Fixture::new(1);
    let second = Fixture::new(2);
    let registry = ProcessorControlRegistry::default();
    let node = NodeId::from("effect");
    let _first = registry.register(&node, first.control());
    first.observed(true);
    first.observed(false);
    let old = registry.editor_close_observation(&node).unwrap();
    first.observed(true);
    first.observed(false);
    assert_eq!(
        registry.snapshot_state_for_editor_close(&node, old, Duration::ZERO),
        Err(ProcessorStateError::Unavailable)
    );
    let latest = registry.editor_close_observation(&node).unwrap();
    let _second = registry.register(&node, second.control());
    second.observed(true);
    second.observed(false);
    assert_ne!(
        latest.instance_id,
        registry
            .editor_close_observation(&node)
            .unwrap()
            .instance_id
    );
    assert_eq!(
        registry.snapshot_state_for_editor_close(&node, latest, Duration::ZERO),
        Err(ProcessorStateError::Unavailable)
    );
    assert!(second.worker.pending_request().is_none());
}

#[test]
fn matching_close_token_exports_native_state_and_stays_retryable() {
    let fixture = Fixture::new(1);
    let states = crate::state_blobs::memory::MemoryBlobs::default();
    let registry = ProcessorControlRegistry::new(Arc::new(states.clone()));
    let node = NodeId::from("effect");
    let _registration = registry.register(&node, fixture.control());
    fixture.observed(true);
    fixture.observed(false);
    let observation = registry.editor_close_observation(&node).unwrap();
    fixture.capture.reply(Ok(vec![1, 2, 3]));
    let state =
        registry.snapshot_state_for_editor_close(&node, observation, Duration::from_secs(1));
    let reference = crate::StateBlobWriter::put(&states, &[1, 2, 3]).unwrap();
    assert_eq!(state.unwrap(), serde_json::json!({"stateRef":reference}));
    assert_eq!(
        registry.editor_close_observation(&node).unwrap(),
        observation
    );
}

#[test]
fn replacement_during_export_cannot_publish_state_into_the_new_instance() {
    let first = Fixture::new(1);
    let replacement = Fixture::new(2);
    let states = crate::state_blobs::memory::MemoryBlobs::default();
    let registry = ProcessorControlRegistry::new(Arc::new(states.clone()));
    let node = NodeId::from("effect");
    let _first_registration = registry.register(&node, first.control());
    first.observed(true);
    first.observed(false);
    let observation = registry.editor_close_observation(&node).unwrap();
    let replacement_control = Arc::clone(replacement.control());
    let worker_registry = registry.clone();
    let worker_node = node.clone();
    let registration = Arc::new(std::sync::Mutex::new(None));
    let retained = Arc::clone(&registration);
    first.capture.reply_with(Ok(vec![7]), move || {
        *retained.lock().unwrap() =
            Some(worker_registry.register(&worker_node, &replacement_control));
    });
    let result =
        registry.snapshot_state_for_editor_close(&node, observation, Duration::from_secs(1));
    drop(registration);
    assert_eq!(result, Err(ProcessorStateError::Unavailable));
}

#[test]
fn close_reopen_close_during_export_does_not_checkpoint_old_reuse_identity() {
    let fixture = Fixture::new(1);
    let states = crate::state_blobs::memory::MemoryBlobs::default();
    let registry = ProcessorControlRegistry::new(Arc::new(states.clone()));
    let node = NodeId::from("effect");
    let _registration = registry.register(&node, fixture.control());
    fixture.control.active_owner.store(true, Ordering::Release);
    fixture.observed(true);
    fixture.observed(false);
    let observation = registry.editor_close_observation(&node).unwrap();

    let config = ProviderNodeConfig {
        ports: None,
        type_id: "fixture:provider".into(),
        parameters: serde_json::json!({"nativeToken":"before-close"}),
    };
    let request = HostedProcessorPrepareRequest {
        node_id: &node,
        config: &config,
        format: RenderFormat {
            sample_rate: 48_000,
            block_frames: 128,
        },
        input_channels: 2,
        output_channels: 2,
    };
    *fixture.control.reuse.lock().unwrap() = Some(crate::reuse::ReuseSignature::new(&request));
    assert_eq!(
        registry.reuse_request(&request).unwrap().reuse_identity(),
        Some(&fixture.control().adoption_identity)
    );

    let worker_control = Arc::clone(&fixture.worker);
    fixture.capture.reply_with(Ok(vec![9, 8, 7]), move || {
        // A second observed close makes the original token stale before capture completes.
        publish_observed_editor_snapshot(
            worker_control.header(),
            rack_formats::EditorSnapshot {
                open: true,
                ..rack_formats::EditorSnapshot::default()
            },
        );
        publish_observed_editor_snapshot(
            worker_control.header(),
            rack_formats::EditorSnapshot::default(),
        );
    });

    let result =
        registry.snapshot_state_for_editor_close(&node, observation, Duration::from_secs(1));

    assert_eq!(result, Err(ProcessorStateError::Unavailable));
    assert_eq!(
        registry.reuse_request(&request).unwrap().reuse_identity(),
        Some(&fixture.control().adoption_identity)
    );
}
