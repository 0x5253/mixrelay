//! Control-thread matching and audio-boundary ownership transfer for surviving plug-ins.

use crate::{HostedProcessorControl, ProcessorControlRegistry, WORKER_READY};
use mixrelay_audio_engine::{
    HostedAudioProcessor, HostedLatencySource, HostedProcessorPrepareRequest,
};
use mixrelay_audio_graph::{ProviderNodeConfig, RenderFormat};
use std::sync::{Arc, Weak, atomic::Ordering};

#[derive(Clone)]
pub(super) struct ReuseSignature {
    config: ProviderNodeConfig,
    format: RenderFormat,
    inputs: u16,
    outputs: u16,
}

/// Control-plane transaction for live workers' candidate reuse metadata.
///
/// The graph owner must serialize capture, preparation and publication while this guard lives.
/// Dropping it restores the original signatures without touching native audio state. Weak
/// references ensure an aborted operation does not keep removed workers alive. Nested guards
/// are valid: an outer failure still restores the state before its first capture.
#[must_use = "keep the guard until graph publication or durable checkpoint acceptance"]
pub struct ProcessorReuseTransaction {
    originals: Vec<(Weak<HostedProcessorControl>, Option<ReuseSignature>)>,
}

impl ProcessorReuseTransaction {
    /// Accepts captured reuse metadata after its owning graph or checkpoint is published.
    /// A portable file export alone does not publish the running graph and must not commit.
    pub fn commit(mut self) {
        self.originals.clear();
    }
}

impl Drop for ProcessorReuseTransaction {
    fn drop(&mut self) {
        for (weak, original) in self.originals.drain(..) {
            if let Some(control) = weak.upgrade() {
                *control
                    .reuse
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = original;
            }
        }
    }
}

impl ReuseSignature {
    pub(super) fn new(request: &HostedProcessorPrepareRequest<'_>) -> Self {
        Self {
            config: request.config.clone(),
            format: request.format,
            inputs: request.input_channels,
            outputs: request.output_channels,
        }
    }

    fn matches(&self, request: &HostedProcessorPrepareRequest<'_>) -> bool {
        self.config == *request.config
            && self.format == request.format
            && self.inputs == request.input_channels
            && self.outputs == request.output_channels
    }

    pub(super) fn checkpoint(&mut self, parameters: &serde_json::Value) {
        self.config.parameters = parameters.clone();
    }
}

/// Inactive placeholder retaining the exact worker selected for ownership transfer.
struct ReuseRequest(Arc<HostedProcessorControl>);

impl HostedAudioProcessor for ReuseRequest {
    fn process(&mut self, _input: &[f32], _output: &mut [f32], _frames: usize) -> bool {
        false
    }

    fn reuse_identity(&self) -> Option<&mixrelay_audio_engine::HostedProcessorIdentity> {
        Some(&self.0.adoption_identity)
    }

    fn requests_reuse(&self) -> bool {
        true
    }

    fn latency_source(&self) -> Option<Arc<dyn HostedLatencySource>> {
        let source: Arc<dyn HostedLatencySource> = Arc::<HostedProcessorControl>::clone(&self.0);
        Some(source)
    }
}

impl ProcessorControlRegistry {
    /// Starts a rollback scope before native state capture or workspace replacement.
    ///
    /// The caller must hold its graph mutation lock until this scope ends. Commit only
    /// after accepted publication; errors and unwinding restore surviving workers' original
    /// reuse identities. This captures metadata only and performs no native or audio work.
    pub fn reuse_transaction(&self) -> ProcessorReuseTransaction {
        let controls = self
            .controls
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let originals = controls
            .values()
            .flatten()
            .filter_map(|weak| {
                let control = weak.upgrade()?;
                let original = control
                    .reuse
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                Some((weak.clone(), original))
            })
            .collect();
        ProcessorReuseTransaction { originals }
    }
    /// Prevents a workspace replacement from inheriting a running native editor's state.
    /// This changes reuse metadata only; active processors continue rendering on failure.
    pub fn invalidate_reuse(&self) {
        let controls = self
            .controls
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for control in controls
            .values()
            .flatten()
            .filter_map(std::sync::Weak::upgrade)
        {
            *control
                .reuse
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
    }
    pub(super) fn reuse_request(
        &self,
        request: &HostedProcessorPrepareRequest<'_>,
    ) -> Option<Box<dyn HostedAudioProcessor>> {
        let control = self.live_control(request.node_id).ok()?;
        let header = control.shared.header();
        if !control.active_owner.load(Ordering::Acquire)
            || header.worker_status.load(Ordering::Acquire) != WORKER_READY
            || header.shutdown.load(Ordering::Acquire) != 0
        {
            return None;
        }
        let signature = control.reuse.lock().ok()?;
        signature
            .as_ref()
            .filter(|signature| signature.matches(request))?;
        drop(signature);
        Some(Box::new(ReuseRequest(control)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixrelay_audio_graph::NodeId;

    #[test]
    fn reuse_requires_matching_type_format_channels_and_current_native_state() {
        let id = NodeId::from("graph-a/effect");
        let config = ProviderNodeConfig {
            ports: None,
            type_id: "au:aufx:bpas:appl".into(),
            parameters: serde_json::json!({"stateRef": {"sha256": "a".repeat(64), "byteLength": 8}}),
        };
        let mut request = HostedProcessorPrepareRequest {
            node_id: &id,
            config: &config,
            format: RenderFormat {
                sample_rate: 48000,
                block_frames: 32,
            },
            input_channels: 2,
            output_channels: 2,
        };
        let mut signature = ReuseSignature::new(&request);
        assert!(signature.matches(&request));
        request.format.sample_rate = 96000;
        assert!(!signature.matches(&request));
        request.format.sample_rate = 48000;
        request.format.block_frames = 64;
        assert!(!signature.matches(&request));
        request.format.block_frames = 32;
        request.input_channels = 1;
        assert!(!signature.matches(&request));
        request.input_channels = 2;
        signature.checkpoint(
            &serde_json::json!({"stateRef": {"sha256": "b".repeat(64), "byteLength": 6}}),
        );
        assert!(!signature.matches(&request));
        let mut updated = config.clone();
        updated.parameters =
            serde_json::json!({"stateRef": {"sha256": "b".repeat(64), "byteLength": 6}});
        request.config = &updated;
        assert!(signature.matches(&request));
        let mut other = updated.clone();
        other.type_id = "vst3:another".into();
        request.config = &other;
        assert!(!signature.matches(&request));
    }

    #[test]
    fn reuse_request_identity_and_latency_reader_share_the_same_worker() {
        use std::sync::atomic::Ordering;

        use crate::{
            AudioLayout, WORKER_READY, latency::publish_intrinsic_frames, shared_transport,
        };
        use mixrelay_test_support::FixtureDir;

        let directory = FixtureDir::new("processor-reuse-latency")
            .expect("reuse latency fixture directory should be created");
        let path = directory.join("audio");
        let layout = AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 64,
        };
        let (parent, mailbox) = shared_transport::create(
            &path,
            layout,
            crate::instance_id::InstanceId::new(1).unwrap(),
        )
        .expect("reuse latency transport should be created");
        let (worker_audio, worker_control) =
            shared_transport::worker(&path).expect("worker mapping should open the fixture");
        let control = Arc::new(HostedProcessorControl::native(
            mailbox,
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        ));
        let header = worker_control.header();
        header.worker_status.store(WORKER_READY, Ordering::Release);
        publish_intrinsic_frames(header, Some(9));
        let identity = &control.adoption_identity;
        let mut request = ReuseRequest(Arc::clone(&control));

        assert_eq!(request.reuse_identity(), Some(identity));
        assert!(request.requests_reuse());
        assert!(!request.process(&[], &mut [], 0));
        let reader = request
            .latency_source()
            .expect("reuse placeholder should retain the worker latency reader");
        assert_eq!(reader.frames(), Some(73));
        publish_intrinsic_frames(header, Some(13));
        assert_eq!(reader.frames(), Some(77));

        drop(request);
        drop(reader);
        drop(control);
        drop(worker_control);
        drop(worker_audio);
        drop(parent);
    }
}
