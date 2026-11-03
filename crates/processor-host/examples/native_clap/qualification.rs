use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use mixrelay_audio_engine::{BlockProcessor, CompensationState, PeakMeterBank, ProcessorExchange};
use mixrelay_audio_graph::{CompileContext, CompileLimits, GraphCompiler, NodeId, RenderFormat};
use mixrelay_processor_host::{
    CatalogNodeResolver, ExternalProcessorFactory, ProcessorCatalog, ProcessorControlRegistry,
};
use mixrelay_processor_state::StateRef;

#[derive(Clone)]
pub(super) struct NativeResources {
    pub(super) registry: ProcessorControlRegistry,
    pub(super) files: mixrelay_processor_host::ProcessorFiles,
    pub(super) clock: Arc<dyn mixrelay_time::MonotonicClock>,
}

impl NativeResources {
    fn new(clock: Arc<dyn mixrelay_time::MonotonicClock>) -> Self {
        let files = mixrelay_processor_host::ProcessorFiles::default();
        let registry = ProcessorControlRegistry::new(Arc::new(files.clone()));
        Self {
            registry,
            files,
            clock,
        }
    }
}

const FRAMES: usize = 128;
const SAMPLES: usize = FRAMES * 2;
const BLOCK_FRAMES: u32 = 128;
const LARGE_STATE_ENV: &str = "MIXRELAY_CLAP_FIXTURE_LARGE_STATE";
const LARGE_NATIVE_BYTES: usize = 32 * 1024 * 1024;
const NATIVE_HEADER_BYTES: usize = 28;
const FORMAT: RenderFormat = RenderFormat {
    sample_rate: 48_000,
    block_frames: BLOCK_FRAMES,
};

struct Harness {
    current: Box<BlockProcessor>,
    exchange: ProcessorExchange,
    controls: NativeResources,
}

impl Harness {
    fn new(
        catalog: ProcessorCatalog,
        controls: NativeResources,
        type_id: &str,
        parameters: serde_json::Value,
    ) -> Result<Self, String> {
        let execution = GraphCompiler::new(CatalogNodeResolver::new(catalog.clone()))
            .compile(
                &super::graph::document(type_id, parameters),
                CompileContext {
                    format: FORMAT,
                    limits: CompileLimits::default(),
                },
            )
            .map_err(|error| error.to_string())?;
        let factory = ExternalProcessorFactory::with_controls(
            catalog,
            controls.registry.clone(),
            controls.files.clone(),
            controls.clock.clone(),
        );
        let current = Box::new(
            BlockProcessor::prepare_with_hosted_processors(
                &execution,
                Arc::new(PeakMeterBank::new(8, 2)),
                &factory,
            )
            .map_err(|error| error.to_string())?,
        );
        let exchange = ProcessorExchange::new(1);
        exchange.observe_plan(Arc::clone(current.plan()));
        Ok(Self {
            current,
            exchange,
            controls,
        })
    }

    fn block(&mut self, input: &[f32; SAMPLES]) -> Result<[f32; SAMPLES], String> {
        self.exchange.refresh_latency()?;
        self.exchange.activate_pending(&mut self.current);
        let mut output = [0.0; SAMPLES];
        self.current
            .process_block(BLOCK_FRAMES, &[input], &mut [&mut output])
            .map_err(|error| error.to_string())?;
        // This is a deterministic IPC/PCM fixture, not a hardware timing measurement.
        std::thread::sleep(Duration::from_millis(10));
        self.exchange.reclaim_retired();
        Ok(output)
    }

    fn settle(&mut self, frames: u32) -> Result<u32, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stable_blocks = 0;
        while Instant::now() < deadline {
            self.block(&[0.0; SAMPLES])?;
            let observed = self
                .controls
                .registry
                .latency_snapshot(&NodeId::from("effect"));
            if let Some(observed) = observed
                && observed.intrinsic_frames == Some(frames)
                && self.current.compensation_state() == CompensationState::Current
            {
                stable_blocks += 1;
                if stable_blocks >= 8 {
                    return Ok(observed.transport_frames + frames);
                }
            } else {
                stable_blocks = 0;
            }
        }
        Err(format!("native latency did not settle to {frames} frames"))
    }

    fn impulse(&mut self, delay: u32, amplitudes: [f32; 2]) -> Result<(), String> {
        let mut input = [0.0; SAMPLES];
        input[0] = 1.0;
        input[1] = 0.5;
        let delay = usize::try_from(delay).map_err(|error| error.to_string())?;
        for block in 0..8 {
            let output = self.block(&input)?;
            for (index, value) in output.iter().copied().enumerate() {
                let frame = block * FRAMES + index / 2;
                let expected = if frame == delay {
                    amplitudes[index % 2]
                } else {
                    0.0
                };
                if !value.is_finite() || (value - expected).abs() > 0.000_001 {
                    return Err(format!(
                        "PCM frame={frame} ch={} got={value} expected={expected}",
                        index % 2
                    ));
                }
            }
            input.fill(0.0);
        }
        Ok(())
    }
}

pub(super) fn run(path: &Path) -> Result<(), String> {
    let large_state = match std::env::var_os(LARGE_STATE_ENV) {
        None => false,
        Some(value) if value == "1" => true,
        Some(_) => return Err(format!("{LARGE_STATE_ENV} must be 1 when set")),
    };
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    let owned = mixrelay_test_support::FixtureDir::new("native-clap-qualification")
        .map_err(|error| error.to_string())?;
    let (catalog, discovered) = discover_fixture(&path)?;
    let descriptor = &discovered.processors[0];
    let controls = NativeResources::new(Arc::new(mixrelay_time::StdClock::new()));
    controls
        .files
        .select(mixrelay_processor_state::StateStore::new(
            owned.join("states"),
        ));
    let mut harness = Harness::new(
        catalog.clone(),
        controls.clone(),
        &descriptor.type_id,
        serde_json::json!({}),
    )?;
    let initial_plan = Arc::clone(harness.current.plan());
    let initial_revisions = harness.exchange.plan_revisions();
    let baseline_delay = harness.settle(0)?;
    harness.impulse(baseline_delay, [2.0, 1.0])?;
    apply_parameters(&controls)?;
    let updated_delay = harness.settle(64)?;
    harness.impulse(updated_delay, [1.5, 0.625])?;
    if !Arc::ptr_eq(&initial_plan, harness.current.plan())
        || harness.exchange.plan_revisions() != initial_revisions
    {
        return Err("parameter/latency update replaced topology or authored revision".into());
    }
    let state = controls
        .registry
        .snapshot_state(&NodeId::from("effect"), Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let reference = state_reference(&state)?;
    if large_state {
        verify_large_state(&controls, &reference)?;
    }
    drop(initial_plan);
    drop(harness);
    if controls.registry.has_worker(&NodeId::from("effect")) {
        return Err("removed fixture worker remains owned".into());
    }
    let mut restored = Harness::new(
        catalog.clone(),
        controls.clone(),
        &descriptor.type_id,
        state.clone(),
    )?;
    let restored_delay = restored.settle(64)?;
    restored.impulse(restored_delay, [1.5, 0.625])?;
    verify_restored_state(&controls, &state)?;
    drop(restored);
    if controls.registry.has_worker(&NodeId::from("effect")) {
        return Err("restored fixture worker remains owned".into());
    }
    super::multibus::run(&catalog, &controls, descriptor)?;
    println!(
        "{}",
        serde_json::json!({
            "fixture": super::dispatch::FIXTURE_ID, "nativeWorker": true,
            "parameters": "independent IDs retained", "initialDelayFrames": baseline_delay,
            "updatedDelayFrames": updated_delay, "restoredDelayFrames": restored_delay,
            "topologyChanges": 0, "pcm": "aligned dry/wet impulses", "cleanup": "released",
            "nativeStateBytes": if large_state { LARGE_NATIVE_BYTES } else { NATIVE_HEADER_BYTES },
            "stateRefByteLength": reference.byte_length(),
            "stateRefSha256": reference.sha256(),
            "freshWorkerStateMatches": true,
            "realTimeTimingQualified": false
        })
    );
    Ok(())
}

fn state_reference(state: &serde_json::Value) -> Result<StateRef, String> {
    let value = state
        .get("stateRef")
        .ok_or("native state snapshot did not return stateRef")?;
    serde_json::from_value(value.clone()).map_err(|error| error.to_string())
}

fn verify_restored_state(
    controls: &NativeResources,
    original: &serde_json::Value,
) -> Result<(), String> {
    let restored = controls
        .registry
        .snapshot_state(&NodeId::from("effect"), Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    if restored != *original {
        return Err("fresh worker exported a different native state".into());
    }
    Ok(())
}

fn verify_large_state(controls: &NativeResources, reference: &StateRef) -> Result<(), String> {
    if reference.byte_length() != LARGE_NATIVE_BYTES as u64 {
        return Err(format!(
            "large stateRef length {} != {LARGE_NATIVE_BYTES}",
            reference.byte_length()
        ));
    }
    let bytes = controls
        .files
        .snapshot()
        .map_err(|error| error.to_string())?
        .read(reference)
        .map_err(|error| error.to_string())?;
    if bytes.len() != LARGE_NATIVE_BYTES {
        return Err("durable native state length differs from stateRef".into());
    }
    let payload = bytes.as_slice();
    if payload[..8] != *b"MRCLAP01"
        || payload[8..16] != 0.5_f64.to_le_bytes()
        || payload[16..24] != 0.25_f64.to_le_bytes()
        || payload[24..28] != 64_u32.to_le_bytes()
    {
        return Err("large native state lost saved gains or delay".into());
    }
    for (offset, byte) in payload[NATIVE_HEADER_BYTES..].iter().copied().enumerate() {
        let index = NATIVE_HEADER_BYTES + offset;
        let expected = (index.wrapping_mul(131) ^ (index >> 9) ^ (index >> 19)).to_le_bytes()[0];
        if byte != expected {
            return Err(format!("large native state mismatch at byte {index}"));
        }
    }
    Ok(())
}

fn apply_parameters(controls: &NativeResources) -> Result<(), String> {
    let node = NodeId::from("effect");
    for step in 0..100_u32 {
        controls
            .registry
            .set_parameter(&node, 1, f64::from(step) / 100.0)?;
        controls
            .registry
            .set_parameter(&node, 2, 1.0 - f64::from(step) / 100.0)?;
    }
    controls.registry.set_parameter(&node, 1, 0.5)?;
    controls.registry.set_parameter(&node, 2, 0.25)?;
    controls.registry.set_parameter(&node, 3, 0.25)
}

fn discover_fixture(
    path: &Path,
) -> Result<
    (
        ProcessorCatalog,
        mixrelay_control_protocol::ExternalProcessorCatalog,
    ),
    String,
> {
    let catalog = ProcessorCatalog::default();
    catalog.rescan(&[path.to_path_buf()]);
    let discovered = catalog.protocol_catalog();
    if !discovered.diagnostics.is_empty() || discovered.processors.len() != 1 {
        return Err(format!(
            "fixture discovery failed: {:?}",
            discovered.diagnostics
        ));
    }
    Ok((catalog, discovered))
}
