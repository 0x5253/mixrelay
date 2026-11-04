use std::{path::PathBuf, sync::Arc, thread, time::Duration};

use mixrelay_audio_graph::{
    AUDIO_PORT_ID, AudioDeviceInputConfig, AudioDeviceOutputConfig, AudioEndpoint,
    BuiltInNodeResolver, ChannelMapping, CompileContext, CompileLimits, EdgeId, EdgeSpec,
    GraphCompiler, GraphDocument, NodeConfig, NodeControl, NodeId, NodeSpec, PortId,
    ProviderNodeConfig, RenderFormat,
};
use mixrelay_audio_io::{
    HostAudioBackendError, VirtualEndpointBackend, VirtualEndpointCapabilities, VirtualEndpointSpec,
};
use mixrelay_control_protocol::{
    AppStatus, DriverState, GraphApplyRequest, ProcessorParameterUpdate,
};
use mixrelay_control_server::ControlBackend;
use mixrelay_engine_core::{FixedRuntimeStatus, default_peak_meter_bank};
use mixrelay_engine_service::controller::{EngineGraphController, EngineGraphControllerConfig};
use mixrelay_render::{BlockProcessor, ProcessorExchange, RenderPlan};

use super::resources::{FixtureResources, SinkOrder};

pub(super) const TARGET_NODE: &str = "target";
pub(super) const HEALTHY_NODE: &str = "healthy";
const TARGET_OUTPUT: &str = "target-output";
const HEALTHY_OUTPUT: &str = "healthy-output";
pub(super) const GRAPH_ID: &str = "graph-1";
const FRAMES: usize = 128;
const BLOCK_FRAMES: u32 = 128;
const SAMPLES: usize = FRAMES * 2;
const FORMAT: RenderFormat = RenderFormat {
    sample_rate: 48_000,
    block_frames: BLOCK_FRAMES,
};
const BLOCK_PERIOD: Duration = Duration::from_millis(10);
const HEALTHY_PCM: (f32, f32) = (1.5, 2.0);
// More blocks than can run within the 90-second deadline at the mandatory 10 ms pace.
const MAX_RENDER_BLOCKS: u16 = 10_000;

pub(super) struct BranchPcm {
    target: Option<[f32; SAMPLES]>,
    healthy: [f32; SAMPLES],
    expected_level: f32,
}

pub(super) struct NativeHarness {
    pub(super) controller: Arc<EngineGraphController>,
    current: Box<BlockProcessor>,
    exchange: Arc<ProcessorExchange>,
    healthy_primed: bool,
    rendered_blocks: u64,
    expected_healthy: Option<(PathBuf, u32, u64)>,
    resources: Arc<FixtureResources>,
}

impl NativeHarness {
    pub(super) fn new(catalog: mixrelay_processor_host::ProcessorCatalog) -> Result<Self, String> {
        let empty = GraphDocument::new(1, vec![], vec![]);
        let compiled = GraphCompiler::new(BuiltInNodeResolver)
            .compile(
                &empty,
                CompileContext {
                    format: FORMAT,
                    limits: CompileLimits::default(),
                },
            )
            .map_err(|error| format!("empty graph compile: {error:?}"))?;
        let current = Box::new(
            BlockProcessor::new(Arc::new(
                RenderPlan::prepare(&compiled)
                    .map_err(|error| format!("empty render plan: {error:?}"))?,
            ))
            .map_err(|error| format!("empty processor storage: {error:?}"))?,
        );
        let exchange = Arc::new(ProcessorExchange::new(1));
        exchange.observe_plan(Arc::clone(current.plan()));
        let resources = Arc::new(FixtureResources::default());
        let controller = Arc::new(
            EngineGraphController::new_with_initial_document(
                Arc::clone(&exchange),
                EngineGraphControllerConfig {
                    clock: std::sync::Arc::new(mixrelay_time::StdClock::new()),
                    format: FORMAT,
                    meters: default_peak_meter_bank(),
                    initial_bindings: vec![],
                    endpoint_backend: Arc::new(NoVirtualEndpoints),
                    required_endpoint_node_id: None,
                    processor_catalog: catalog,
                    graph_resources: resources.clone(),
                    runtime_status: Arc::new(FixedRuntimeStatus::new(DriverState::Ready)),
                },
                empty,
            )
            .with_graph_catalog(),
        );
        Ok(Self {
            controller,
            current,
            exchange,
            healthy_primed: false,
            rendered_blocks: 0,
            expected_healthy: None,
            resources,
        })
    }

    pub(super) fn apply(&mut self, mut document: GraphDocument) -> Result<u64, String> {
        document.revision = self
            .graph_revision()
            .checked_add(1)
            .ok_or("graph revision overflow")?;
        let response = ControlBackend::apply_graph_request(
            self.controller.as_ref(),
            GraphApplyRequest::from(document),
        )
        .map_err(|error| format!("graph publication: {error:?}"))?;
        self.activate_revision(response.revision)?;
        if response.state != mixrelay_control_protocol::GraphApplyState::Pending {
            return Err("graph apply did not use pending publication".into());
        }
        Ok(response.revision)
    }

    pub(super) fn set_parameter(&self, node: &str, id: u32, value: f64) -> Result<(), String> {
        let revision = ControlBackend::graph_status(self.controller.as_ref()).submitted_revision;
        ControlBackend::set_processor_parameter(
            self.controller.as_ref(),
            node,
            id,
            ProcessorParameterUpdate {
                expected_revision: revision,
                value,
            },
        )
        .map(|_| ())
        .map_err(|error| format!("parameter update {node}/{id}: {error:?}"))
    }

    pub(super) fn checkpoint(&self) -> Result<(), String> {
        self.controller
            .checkpoint_processor_state(None)
            .map_err(|error| format!("native state checkpoint: {error:?}"))
    }

    pub(super) fn status(&self) -> AppStatus {
        ControlBackend::status(self.controller.as_ref())
    }

    pub(super) fn graph(&self) -> GraphDocument {
        ControlBackend::graph(self.controller.as_ref())
    }

    pub(super) fn graph_revision(&self) -> u64 {
        ControlBackend::graph_status(self.controller.as_ref()).submitted_revision
    }

    pub(super) fn catalog_revision(&self) -> Result<u64, String> {
        ControlBackend::graph_catalog(self.controller.as_ref())
            .map(|catalog| catalog.catalog_revision)
            .map_err(|error| format!("graph catalog status: {error:?}"))
    }

    pub(super) fn block(&mut self) -> Result<BranchPcm, String> {
        self.exchange.refresh_latency()?;
        self.exchange.activate_pending(&mut self.current);
        let input_level = block_level(self.rendered_blocks)?;
        let expected_level = block_level(self.rendered_blocks.saturating_sub(1))?;
        let input = constant_stereo(2.0 * input_level, 4.0 * input_level);
        let layout = self
            .resources
            .sink_order(self.current.plan().sink_bindings())?;
        let mut target = [0.0; SAMPLES];
        let mut healthy = [0.0; SAMPLES];
        let target_present = match layout {
            SinkOrder::TargetFirst => {
                self.current
                    .process_block(BLOCK_FRAMES, &[&input], &mut [&mut target, &mut healthy])
                    .map_err(|error| format!("native fixture render: {error:?}"))?;
                true
            }
            SinkOrder::HealthyFirst => {
                self.current
                    .process_block(BLOCK_FRAMES, &[&input], &mut [&mut healthy, &mut target])
                    .map_err(|error| format!("native fixture render: {error:?}"))?;
                true
            }
            SinkOrder::HealthyOnly => {
                self.current
                    .process_block(BLOCK_FRAMES, &[&input], &mut [&mut healthy])
                    .map_err(|error| format!("native fixture render: {error:?}"))?;
                false
            }
        };
        self.exchange.reclaim_retired();
        thread::sleep(BLOCK_PERIOD);
        self.rendered_blocks = self.rendered_blocks.saturating_add(1);
        if self.healthy_primed {
            check_pcm(&healthy, scaled(HEALTHY_PCM, expected_level), HEALTHY_NODE)
                .map_err(|error| format!("render block {}: {error}", self.rendered_blocks))?;
        }
        self.check_healthy_worker()?;
        Ok(BranchPcm {
            target: target_present.then_some(target),
            healthy,
            expected_level,
        })
    }

    pub(super) fn expect_healthy_worker(
        &mut self,
        private_dir: PathBuf,
        expected_pid: u32,
    ) -> Result<(), String> {
        let heartbeat = super::worker::wait_heartbeat(
            &private_dir,
            super::worker::WorkerRole::Healthy,
            Duration::from_secs(1),
        )?;
        if heartbeat.pid != expected_pid {
            return Err(format!(
                "healthy worker PID changed before monitoring: expected={expected_pid} got={}",
                heartbeat.pid
            ));
        }
        self.expected_healthy = Some((private_dir, expected_pid, heartbeat.sequence));
        Ok(())
    }

    pub(super) fn settle_and_check_branches(
        &mut self,
        target_expected: Option<(f32, f32)>,
        healthy_expected: (f32, f32),
    ) -> Result<(), String> {
        let mut last = BranchPcm {
            target: None,
            healthy: [0.0; SAMPLES],
            expected_level: 1.0,
        };
        for _ in 0..12 {
            last = self.block()?;
        }
        if let Some(expected) = target_expected {
            let target = last.target.ok_or("target output binding is absent")?;
            check_pcm(&target, scaled(expected, last.expected_level), TARGET_NODE)?;
        }
        check_pcm(
            &last.healthy,
            scaled(healthy_expected, last.expected_level),
            HEALTHY_NODE,
        )?;
        Ok(())
    }

    pub(super) fn prime_healthy_branch(&mut self) -> Result<(), String> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut stable = 0;
        while std::time::Instant::now() < deadline {
            let actual = self.block()?;
            if pcm_matches(&actual.healthy, scaled(HEALTHY_PCM, actual.expected_level)) {
                stable += 1;
                if stable >= 3 {
                    self.healthy_primed = true;
                    return Ok(());
                }
            } else {
                stable = 0;
            }
        }
        Err("healthy branch did not reach stable PCM before the recovery test".into())
    }

    pub(super) fn activate_revision(&mut self, revision: u64) -> Result<(), String> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let _ = self.block()?;
            let status = ControlBackend::graph_status(self.controller.as_ref());
            if status.active_revision == revision && status.submitted_revision == revision {
                return Ok(());
            }
        }
        Err(format!("graph revision {revision} did not activate"))
    }

    pub(super) fn shutdown(self) {
        let Self {
            controller,
            current,
            exchange,
            ..
        } = self;
        drop(current);
        exchange.reclaim_retired();
        drop(controller);
        exchange.reclaim_retired();
        drop(exchange);
    }

    fn check_healthy_worker(&mut self) -> Result<(), String> {
        let Some((directory, expected_pid, previous_sequence)) = &mut self.expected_healthy else {
            return Ok(());
        };
        let heartbeat = super::worker::wait_heartbeat(
            directory,
            super::worker::WorkerRole::Healthy,
            Duration::from_millis(200),
        )?;
        if heartbeat.pid != *expected_pid || heartbeat.sequence < *previous_sequence {
            return Err(format!(
                "healthy worker identity regressed at render block {}: expected PID {expected_pid} sequence >= {previous_sequence}, got PID {} sequence {}",
                self.rendered_blocks, heartbeat.pid, heartbeat.sequence
            ));
        }
        *previous_sequence = heartbeat.sequence;
        Ok(())
    }
}

#[derive(Default)]
struct NoVirtualEndpoints;

impl VirtualEndpointBackend for NoVirtualEndpoints {
    fn capabilities(&self) -> &VirtualEndpointCapabilities {
        static CAPABILITIES: std::sync::OnceLock<VirtualEndpointCapabilities> =
            std::sync::OnceLock::new();
        CAPABILITIES.get_or_init(|| VirtualEndpointCapabilities {
            max_render_channels: 2,
            max_capture_channels: 2,
            supported_sample_rates: vec![48_000],
            supports_duplex: false,
            supports_dynamic_creation: false,
        })
    }

    fn prepare_graph_attachments(
        &self,
        endpoints: &[VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError> {
        if endpoints.is_empty() {
            Ok(())
        } else {
            Err(HostAudioBackendError::CapabilityUnavailable {
                capability: "virtual devices are forbidden in recovery qualification".into(),
            })
        }
    }
}

pub(super) fn graph(type_id: &str, target: serde_json::Value) -> GraphDocument {
    let healthy = serde_json::json!({"parameterValues":{"1":0.75,"2":0.5,"3":0.0}});
    GraphDocument::new(
        2,
        vec![
            node(
                "input",
                NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                    selector: super::resources::selector(super::resources::INPUT_UID),
                    output_channels: 2,
                    gain_db: 0.0,
                }),
            ),
            provider(TARGET_NODE, type_id, target),
            node(
                TARGET_OUTPUT,
                NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                    selector: super::resources::selector(super::resources::TARGET_UID),
                    input_channels: 2,
                }),
            ),
            provider(HEALTHY_NODE, type_id, healthy),
            node(
                HEALTHY_OUTPUT,
                NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                    selector: super::resources::selector(super::resources::HEALTHY_UID),
                    input_channels: 2,
                }),
            ),
        ],
        vec![
            edge("input-target", "input", TARGET_NODE),
            edge("target-output-edge", TARGET_NODE, TARGET_OUTPUT),
            edge("input-healthy", "input", HEALTHY_NODE),
            edge("healthy-output-edge", HEALTHY_NODE, HEALTHY_OUTPUT),
        ],
    )
}

pub(super) fn target_parameters(document: &GraphDocument) -> Result<serde_json::Value, String> {
    provider_parameters(document, TARGET_NODE)
}

pub(super) fn healthy_parameters(document: &GraphDocument) -> Result<serde_json::Value, String> {
    provider_parameters(document, HEALTHY_NODE)
}

pub(super) fn with_target_enabled(
    mut document: GraphDocument,
    enabled: bool,
) -> Result<GraphDocument, String> {
    let node = document
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == TARGET_NODE)
        .ok_or("target node is absent")?;
    node.control.enabled = enabled;
    Ok(document)
}

pub(super) fn without_target(mut document: GraphDocument) -> GraphDocument {
    document
        .nodes
        .retain(|node| node.id.as_str() != TARGET_NODE && node.id.as_str() != TARGET_OUTPUT);
    document.edges.retain(|edge| {
        edge.source.node_id.as_str() != TARGET_NODE
            && edge.destination.node_id.as_str() != TARGET_NODE
            && edge.source.node_id.as_str() != TARGET_OUTPUT
            && edge.destination.node_id.as_str() != TARGET_OUTPUT
    });
    document
}

pub(super) fn provider_parameters(
    document: &GraphDocument,
    id: &str,
) -> Result<serde_json::Value, String> {
    document
        .nodes
        .iter()
        .find(|node| node.id.as_str() == id)
        .and_then(|node| match &node.config {
            NodeConfig::Provider(provider) => Some(provider.parameters.clone()),
            _ => None,
        })
        .ok_or_else(|| format!("provider {id} is absent"))
}

fn provider(id: &str, type_id: &str, parameters: serde_json::Value) -> NodeSpec {
    node(
        id,
        NodeConfig::Provider(ProviderNodeConfig {
            ports: None,
            type_id: type_id.into(),
            parameters,
        }),
    )
}

fn node(id: &str, config: NodeConfig) -> NodeSpec {
    NodeSpec {
        id: NodeId::from(id),
        label: None,
        control: NodeControl::default(),
        config,
    }
}

fn edge(id: &str, source: &str, destination: &str) -> EdgeSpec {
    EdgeSpec {
        id: EdgeId::from(id),
        source: AudioEndpoint {
            node_id: source.into(),
            port_id: PortId::from(AUDIO_PORT_ID),
        },
        destination: AudioEndpoint {
            node_id: destination.into(),
            port_id: PortId::from(AUDIO_PORT_ID),
        },
        channel_mapping: ChannelMapping::Auto,
    }
}

pub(super) fn constant_stereo(left: f32, right: f32) -> [f32; SAMPLES] {
    let mut samples = [0.0; SAMPLES];
    for frame in samples.chunks_exact_mut(2) {
        frame[0] = left;
        frame[1] = right;
    }
    samples
}

// Each block carries a different exactly representable level. Constant input would
// miss a worker replaying a stale output block during another processor's recovery.
fn block_level(sequence: u64) -> Result<f32, String> {
    let sequence = u16::try_from(sequence)
        .ok()
        .filter(|sequence| *sequence <= MAX_RENDER_BLOCKS)
        .ok_or("qualification exceeded its nonrepeating PCM block budget")?;
    Ok(1.0 + f32::from(sequence) / 8192.0)
}

fn scaled(amplitudes: (f32, f32), level: f32) -> (f32, f32) {
    (amplitudes.0 * level, amplitudes.1 * level)
}

fn check_pcm(actual: &[f32; SAMPLES], expected: (f32, f32), node: &str) -> Result<(), String> {
    for (index, actual) in actual.iter().copied().enumerate() {
        let channel_expected = if index % 2 == 0 {
            expected.0
        } else {
            expected.1
        };
        if !actual.is_finite() || (actual - channel_expected).abs() > 0.000_01 {
            return Err(format!(
                "{node} PCM mismatch sample={index} got={actual} expected={channel_expected}"
            ));
        }
    }
    Ok(())
}

fn pcm_matches(actual: &[f32; SAMPLES], expected: (f32, f32)) -> bool {
    actual.iter().copied().enumerate().all(|(index, sample)| {
        let expected = if index % 2 == 0 {
            expected.0
        } else {
            expected.1
        };
        sample.is_finite() && (sample - expected).abs() <= 0.000_01
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_pcm_check_rejects_replay_throughout_the_bounded_run() {
        for sequence in 1..=u64::from(MAX_RENDER_BLOCKS) {
            let before = block_level(sequence - 1).unwrap();
            let current = block_level(sequence).unwrap();
            assert!(current - before > 0.000_01);
        }
        for offset in [1, 32, 9_000, u64::from(MAX_RENDER_BLOCKS)] {
            let previous = scaled(HEALTHY_PCM, block_level(0).unwrap());
            let expected = scaled(HEALTHY_PCM, block_level(offset).unwrap());
            let replayed = constant_stereo(previous.0, previous.1);
            let current = constant_stereo(expected.0, expected.1);
            assert!(check_pcm(&current, expected, HEALTHY_NODE).is_ok());
            assert!(check_pcm(&replayed, expected, HEALTHY_NODE).is_err());
        }
        assert!(block_level(u64::from(MAX_RENDER_BLOCKS) + 1).is_err());
        assert!(block_level(u64::MAX).is_err());
    }
}
