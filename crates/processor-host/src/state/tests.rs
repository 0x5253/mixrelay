use super::*;
use crate::{
    StateBlobWriter as _, state_blobs::memory::MemoryBlobs, state_capture::memory::MemoryCapture,
};

#[test]
fn missing_state_or_unconfigured_store_cannot_start_with_default_state() {
    let store = MemoryBlobs::default();
    let reference = store.put(&[1, 2, 3]).unwrap();
    let parameters = serde_json::json!({"stateRef":reference});
    assert!(InitialState::resolve(parameters.clone(), None).is_err());
    let empty = MemoryBlobs::default();
    assert!(InitialState::resolve(parameters, Some(&empty)).is_err());
    for value in [
        Value::Null,
        serde_json::json!({"sha256":"../escape","byteLength":3}),
    ] {
        assert!(
            InitialState::resolve(serde_json::json!({"stateRef":value}), Some(&store)).is_err()
        );
    }
}

#[test]
fn capture_interns_before_reuse_matching_and_keeps_json_small() {
    let fixture = StateCaptureFixture::new();
    let transaction = fixture.registry.reuse_transaction();
    let parameters = fixture.capture(vec![0x5a; 9 * 1024 * 1024]);
    assert!(serde_json::to_vec(&parameters).unwrap().len() < 200);
    fixture.registry.verify_state(&parameters).unwrap();
    let candidate = fixture.candidate(parameters);
    assert_eq!(
        fixture.reuse_identity(&candidate),
        Some(fixture.instance_id())
    );
    transaction.commit();
}

struct StateCaptureFixture {
    control: Option<std::sync::Arc<HostedProcessorControl>>,
    worker: std::sync::Arc<crate::shared_transport::WorkerControl>,
    registration: Option<crate::control::ProcessorControlRegistration>,
    _audio: crate::shared_transport::ParentAudio,
    registry: ProcessorControlRegistry,
    states: MemoryBlobs,
    capture: MemoryCapture,
    node: crate::NodeId,
    config: mixrelay_audio_graph::ProviderNodeConfig,
    format: mixrelay_audio_graph::RenderFormat,
}

impl StateCaptureFixture {
    fn new() -> Self {
        use crate::{AudioLayout, NodeId};
        use mixrelay_audio_graph::{ProviderNodeConfig, RenderFormat};
        use std::sync::{Arc, atomic::Ordering};

        let (audio, mailbox, _worker_audio, worker) = crate::shared_transport::memory::endpoints(
            AudioLayout {
                inputs: 2,
                outputs: 2,
                block_frames: 128,
            },
            crate::instance_id::InstanceId::new(1).unwrap(),
        );
        let capture = MemoryCapture::default();
        let control = Arc::new(HostedProcessorControl::new(
            Arc::new(mailbox),
            Arc::new(mixrelay_time::ManualClock::default()),
            Box::new(capture.clone()),
        ));
        control.active_owner.store(true, Ordering::Release);
        control
            .shared
            .header()
            .worker_status
            .store(crate::WORKER_READY, Ordering::Release);
        let node = NodeId::from("effect");
        let states = MemoryBlobs::default();
        let registry = ProcessorControlRegistry::new(Arc::new(states.clone()));
        let registration = registry.register(&node, &control);
        let config = ProviderNodeConfig {
            type_id: "vst3:fixture".into(),
            ports: None,
            parameters: serde_json::json!({"nativeToken":"original"}),
        };
        let format = RenderFormat {
            sample_rate: 48_000,
            block_frames: 128,
        };
        let request = mixrelay_render::HostedProcessorPrepareRequest {
            node_id: &node,
            config: &config,
            format,
            input_channels: 2,
            output_channels: 2,
        };
        *control.reuse.lock().unwrap() = Some(crate::reuse::ReuseSignature::new(&request));

        Self {
            control: Some(control),
            worker: Arc::new(worker),
            registration: Some(registration),
            _audio: audio,
            registry,
            states,
            capture,
            node,
            config,
            format,
        }
    }

    fn instance_id(&self) -> mixrelay_render::HostedProcessorIdentity {
        self.control.as_ref().unwrap().adoption_identity.clone()
    }

    fn candidate(&self, parameters: Value) -> mixrelay_audio_graph::ProviderNodeConfig {
        let mut candidate = self.config.clone();
        candidate.parameters = parameters;
        candidate
    }

    fn reuse_identity(
        &self,
        config: &mixrelay_audio_graph::ProviderNodeConfig,
    ) -> Option<mixrelay_render::HostedProcessorIdentity> {
        let request = mixrelay_render::HostedProcessorPrepareRequest {
            node_id: &self.node,
            config,
            format: self.format,
            input_channels: 2,
            output_channels: 2,
        };
        self.registry
            .reuse_request(&request)
            .and_then(|processor| processor.reuse_identity().cloned())
    }

    fn capture(&self, bytes: Vec<u8>) -> Value {
        self.capture_result(bytes).unwrap()
    }

    fn capture_result(&self, bytes: Vec<u8>) -> Result<Value, ProcessorStateError> {
        self.capture.reply(Ok(bytes));
        self.registry
            .snapshot_state(&self.node, Duration::from_secs(5))
    }
}

#[test]
fn failed_or_export_only_capture_rolls_back_candidate_reuse() {
    let fixture = StateCaptureFixture::new();
    let old_identity = fixture.instance_id();
    let mut failed_candidate = None;
    let mut prepare = || -> Result<(), &str> {
        let _transaction = fixture.registry.reuse_transaction();
        let candidate = fixture.candidate(fixture.capture(vec![4, 5, 6]));
        assert_eq!(
            fixture.reuse_identity(&candidate),
            Some(old_identity.clone())
        );
        failed_candidate = Some(candidate);
        Err("forced graph preparation failure")
    };
    let failure = prepare();
    assert_eq!(failure, Err("forced graph preparation failure"));
    let failed_candidate = failed_candidate.unwrap();

    assert_eq!(
        fixture.reuse_identity(&fixture.config),
        Some(old_identity.clone())
    );
    assert_eq!(fixture.reuse_identity(&failed_candidate), None);

    let transaction = fixture.registry.reuse_transaction();
    let exported_only = fixture.candidate(fixture.capture(vec![1, 2, 3]));
    assert_eq!(
        fixture.reuse_identity(&exported_only),
        Some(old_identity.clone())
    );
    drop(transaction);

    assert_eq!(
        fixture.reuse_identity(&fixture.config),
        Some(old_identity.clone())
    );
    assert_eq!(fixture.reuse_identity(&exported_only), None);
}

#[test]
fn committed_capture_promotes_candidate_reuse_signature() {
    let fixture = StateCaptureFixture::new();
    let old_identity = fixture.instance_id();
    let transaction = fixture.registry.reuse_transaction();
    let candidate = fixture.candidate(fixture.capture(vec![7, 8, 9]));

    transaction.commit();

    assert_eq!(fixture.reuse_identity(&fixture.config), None);
    assert_eq!(
        fixture.reuse_identity(&candidate),
        Some(old_identity.clone())
    );
}

#[test]
fn inner_commit_remains_reversible_by_outer_reuse_transaction() {
    let fixture = StateCaptureFixture::new();
    let old_identity = fixture.instance_id();
    let outer = fixture.registry.reuse_transaction();
    let first = fixture.candidate(fixture.capture(vec![1, 2, 3]));
    let inner = fixture.registry.reuse_transaction();
    let second = fixture.candidate(fixture.capture(vec![4, 5, 6]));

    inner.commit();
    assert_eq!(fixture.reuse_identity(&second), Some(old_identity.clone()));
    drop(outer);

    assert_eq!(
        fixture.reuse_identity(&fixture.config),
        Some(old_identity.clone())
    );
    assert_eq!(fixture.reuse_identity(&first), None);
    assert_eq!(fixture.reuse_identity(&second), None);
}

#[test]
fn reuse_transaction_weak_references_do_not_retain_removed_controls() {
    let mut fixture = StateCaptureFixture::new();
    let control = fixture.control.as_ref().unwrap();
    let weak = std::sync::Arc::downgrade(control);
    let transaction = fixture.registry.reuse_transaction();

    drop(fixture.registration.take());
    drop(fixture.control.take());

    assert!(weak.upgrade().is_none());
    drop(transaction);
    assert!(weak.upgrade().is_none());
}

#[test]
fn unavailable_storage_rejects_capture_before_requesting_native_state() {
    let fixture = StateCaptureFixture::new();
    let registry = ProcessorControlRegistry::default();
    let _registration = registry.register(&fixture.node, fixture.control.as_ref().unwrap());

    assert_eq!(
        registry.snapshot_state(&fixture.node, Duration::ZERO),
        Err(ProcessorStateError::Storage)
    );
    assert!(fixture.worker.pending_request().is_none());
    assert_eq!(fixture.capture.calls(), 0);
    assert_eq!(
        fixture.reuse_identity(&fixture.config),
        Some(fixture.instance_id())
    );
}

#[test]
fn failed_blob_write_does_not_checkpoint_the_captured_native_state() {
    let fixture = StateCaptureFixture::new();
    let transaction = fixture.registry.reuse_transaction();
    fixture.states.reject_writes();

    assert_eq!(
        fixture.capture_result(vec![7, 8, 9]),
        Err(ProcessorStateError::Storage)
    );
    assert_eq!(
        fixture.reuse_identity(&fixture.config),
        Some(fixture.instance_id())
    );
    assert!(fixture.worker.pending_request().is_none());
    assert_eq!(fixture.capture.calls(), 1);
    transaction.commit();
    assert_eq!(
        fixture.reuse_identity(&fixture.config),
        Some(fixture.instance_id())
    );
}
