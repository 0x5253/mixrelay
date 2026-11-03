use std::{
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use mixrelay_control_protocol::{
    ErrorCode, FailedProcessor, GraphApplyResponse, GraphApplyState, ProcessorRetryRequest,
};
use mixrelay_control_server::ControlBackend;
use mixrelay_processor_host::ProcessorCatalog;
use mixrelay_processor_state::{StateRef, StateStore};
use serde_json::Value;

use super::{
    dispatch::{FIXTURE_ENV, FIXTURE_ID, PRIVATE_ENV},
    harness::{
        GRAPH_ID, HEALTHY_NODE, NativeHarness, TARGET_NODE, graph, healthy_parameters,
        target_parameters, with_target_enabled, without_target,
    },
    worker::{self, Heartbeat, WorkerRole},
};

const TARGET_PARAMETERS: (f32, f32) = (1.0, 1.0);
const HEALTHY_PARAMETERS: (f32, f32) = (1.5, 2.0);
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

struct Qualification {
    private_dir: PathBuf,
    harness: NativeHarness,
    healthy_pid: u32,
    target_heartbeat: Heartbeat,
    first_failure: FailedProcessor,
    target_state: StateRef,
    healthy_state: StateRef,
}

pub(super) fn run() -> Result<(), String> {
    let mut test = Qualification::prepare()?;
    test.qualify_retry_and_committed_state()?;
    test.qualify_off_and_removed_target_rejection()?;
    test.verify_cleanup()
}

impl Qualification {
    fn prepare() -> Result<Self, String> {
        let fixture = configured_path(FIXTURE_ENV)?;
        let private_dir = configured_path(PRIVATE_ENV)?;
        let (catalog, type_id) = discover_fixture(&fixture)?;
        let initial_target = serde_json::json!({"parameterValues":{"1":0.5,"2":0.25,"3":0.0}});
        let initial_healthy = serde_json::json!({"parameterValues":{"1":0.75,"2":0.5,"3":0.0}});
        worker::add_role(&private_dir, &initial_target, WorkerRole::Target)?;
        worker::add_role(&private_dir, &initial_healthy, WorkerRole::Healthy)?;

        let mut harness = NativeHarness::new(catalog)?;
        harness.apply(graph(&type_id, initial_target))?;
        set_initial_parameters(&harness)?;
        // Process-required CLAP parameter events must reach the DSP thread before export.
        harness.settle_and_check_branches(Some(TARGET_PARAMETERS), HEALTHY_PARAMETERS)?;
        harness.checkpoint()?;
        let saved = harness.graph();
        let target_parameters = target_parameters(&saved)?;
        let healthy_parameters = healthy_parameters(&saved)?;
        let config_dir = private_config_dir()?;
        let target_state = verify_state(&target_parameters, &config_dir)?;
        let healthy_state = verify_state(&healthy_parameters, &config_dir)?;
        worker::add_role(&private_dir, &target_parameters, WorkerRole::Target)?;
        worker::add_role(&private_dir, &healthy_parameters, WorkerRole::Healthy)?;

        harness.settle_and_check_branches(Some(TARGET_PARAMETERS), HEALTHY_PARAMETERS)?;
        harness.prime_healthy_branch()?;
        let target_heartbeat =
            worker::wait_heartbeat(&private_dir, WorkerRole::Target, EVENT_TIMEOUT)?;
        let healthy_heartbeat =
            worker::wait_heartbeat(&private_dir, WorkerRole::Healthy, EVENT_TIMEOUT)?;
        harness.expect_healthy_worker(private_dir.clone(), healthy_heartbeat.pid)?;
        let first_failure = trigger_failure(&mut harness, &private_dir, target_heartbeat)?;
        Ok(Self {
            private_dir,
            harness,
            healthy_pid: healthy_heartbeat.pid,
            target_heartbeat,
            first_failure,
            target_state,
            healthy_state,
        })
    }

    fn qualify_retry_and_committed_state(&mut self) -> Result<(), String> {
        let revision = self.harness.graph_revision();
        let stale = retry_request(
            &self.harness,
            &self.first_failure,
            revision.saturating_sub(1),
        )?;
        let stale_result = send_retry(&self.harness, stale);
        expect_error(
            stale_result,
            ErrorCode::GraphRevisionStale,
            "stale revision",
        )?;
        let request = retry_request(
            &self.harness,
            &self.first_failure,
            self.harness.graph_revision(),
        )?;
        let response =
            ControlBackend::retry_failed_processor(self.harness.controller.as_ref(), request)
                .map_err(|error| format!("explicit processor retry: {error:?}"))?;
        if response.state != GraphApplyState::Pending {
            return Err("processor retry did not publish a pending graph revision".into());
        }
        self.harness.activate_revision(response.revision)?;
        self.harness
            .settle_and_check_branches(Some(TARGET_PARAMETERS), HEALTHY_PARAMETERS)?;
        let replacement = wait_new_worker(&self.private_dir, self.target_heartbeat, EVENT_TIMEOUT)?;
        if replacement.pid == self.target_heartbeat.pid
            && replacement.sequence >= self.target_heartbeat.sequence
        {
            return Err("explicit retry did not create a new target worker".into());
        }
        self.target_heartbeat = replacement;
        if self
            .harness
            .status()
            .failed_processors
            .iter()
            .any(is_target)
        {
            return Err("retried target is still reported failed".into());
        }

        let stale_instance = retry_request(
            &self.harness,
            &self.first_failure,
            self.harness.graph_revision(),
        )?;
        expect_error(
            send_retry(&self.harness, stale_instance),
            ErrorCode::ProcessorUnavailable,
            "replaced worker token",
        )?;
        let config_dir = private_config_dir()?;
        let restored = self.harness.graph();
        if verify_state(&target_parameters(&restored)?, &config_dir)? != self.target_state
            || verify_state(&healthy_parameters(&restored)?, &config_dir)? != self.healthy_state
        {
            return Err("recovery replaced a committed native state reference".into());
        }
        Ok(())
    }

    fn qualify_off_and_removed_target_rejection(&mut self) -> Result<(), String> {
        let disabled_document = with_target_enabled(self.harness.graph(), false)?;
        self.harness.apply(disabled_document)?;
        if self
            .harness
            .status()
            .failed_processors
            .iter()
            .any(is_target)
        {
            return Err("OFF target remained in active failed-processor status".into());
        }
        let disabled = retry_request(
            &self.harness,
            &self.first_failure,
            self.harness.graph_revision(),
        )?;
        expect_error(
            send_retry(&self.harness, disabled),
            ErrorCode::NodeDisabled,
            "OFF retry",
        )?;
        let removed_document = without_target(self.harness.graph());
        self.harness.apply(removed_document)?;
        let removed_retry = retry_request(
            &self.harness,
            &self.first_failure,
            self.harness.graph_revision(),
        )?;
        expect_error(
            send_retry(&self.harness, removed_retry),
            ErrorCode::NodeNotFound,
            "removed target retry",
        )?;
        Ok(())
    }

    fn verify_cleanup(mut self) -> Result<(), String> {
        self.harness
            .settle_and_check_branches(None, HEALTHY_PARAMETERS)?;
        let latest = worker::wait_heartbeat(&self.private_dir, WorkerRole::Healthy, EVENT_TIMEOUT)?;
        if latest.pid != self.healthy_pid {
            return Err("healthy worker PID changed during target removal".into());
        }
        let private_dir = self.private_dir.clone();
        let receipt = serde_json::json!({
            "fixture": FIXTURE_ID,
            "targetFailedInstance": self.first_failure.instance_id,
            "targetReplacementPid": self.target_heartbeat.pid,
            "healthyPid": self.healthy_pid,
                "healthyPcm": "sequence-varying stereo PCM within 1e-5 on every block after priming",
            "targetPcm": "restored from committed native checkpoint",
            "targetStateSha256": self.target_state.sha256(),
            "healthyStateSha256": self.healthy_state.sha256(),
            "offDeleteStaleGuards": "passed",
            "realtimeTimingQualified": false
        });
        self.harness.shutdown();
        verify_heartbeat_stopped(&private_dir, latest)?;
        println!("{receipt}");
        Ok(())
    }
}

fn discover_fixture(path: &std::path::Path) -> Result<(ProcessorCatalog, String), String> {
    let catalog = ProcessorCatalog::default();
    catalog.rescan(&[path.to_path_buf()]);
    let snapshot = catalog.protocol_catalog();
    let matches = snapshot
        .processors
        .iter()
        .filter(|descriptor| descriptor.type_id == format!("clap:{FIXTURE_ID}"))
        .collect::<Vec<_>>();
    if !snapshot.diagnostics.is_empty() || matches.len() != 1 {
        return Err(format!(
            "explicit native fixture discovery failed: diagnostics={:?}, matchingDescriptors={}",
            snapshot.diagnostics,
            matches.len()
        ));
    }
    let descriptor = matches[0];
    if !descriptor.layouts.iter().any(|ports| {
        ports.layout_id == descriptor.default_layout_id
            && ports.total_input_channels() == Ok(2)
            && ports.total_output_channels() == Ok(2)
    }) {
        return Err("native fixture is not a stereo effect".into());
    }
    for id in [1, 2, 3] {
        if !descriptor
            .parameters
            .iter()
            .any(|parameter| parameter.id == id && !parameter.is_read_only)
        {
            return Err(format!("native fixture parameter {id} is unavailable"));
        }
    }
    Ok((catalog, descriptor.type_id.clone()))
}

fn set_initial_parameters(harness: &NativeHarness) -> Result<(), String> {
    for (node, values) in [(TARGET_NODE, [0.5, 0.25]), (HEALTHY_NODE, [0.75, 0.5])] {
        harness.set_parameter(node, 1, values[0])?;
        harness.set_parameter(node, 2, values[1])?;
        harness.set_parameter(node, 3, 0.0)?;
    }
    Ok(())
}

fn trigger_failure(
    harness: &mut NativeHarness,
    private_dir: &std::path::Path,
    expected: Heartbeat,
) -> Result<FailedProcessor, String> {
    let marker = private_dir.join("target-fault.request");
    let consumed = private_dir.join("target-fault.consumed");
    if marker.exists() || consumed.exists() {
        return Err("target fault marker is not in a clean state".into());
    }
    let nonce = fault_nonce();
    std::fs::write(&marker, &nonce).map_err(|error| error.to_string())?;
    let deadline = Instant::now() + EVENT_TIMEOUT;
    while Instant::now() < deadline {
        harness.block()?;
        if let Some(pid) = read_consumed_nonce(&consumed, &nonce)
            && pid == expected.pid
            && let Some(failure) = failed_target(harness)
        {
            std::fs::remove_file(&consumed).map_err(|error| error.to_string())?;
            return Ok(failure);
        }
    }
    Err("target worker did not fail after consuming its explicit fault marker".into())
}

fn failed_target(harness: &NativeHarness) -> Option<FailedProcessor> {
    harness
        .status()
        .failed_processors
        .into_iter()
        .find(is_target)
}

fn is_target(failure: &FailedProcessor) -> bool {
    failure.graph_id == GRAPH_ID && failure.node_id == TARGET_NODE
}

fn retry_request(
    harness: &NativeHarness,
    failed: &FailedProcessor,
    expected_revision: u64,
) -> Result<ProcessorRetryRequest, String> {
    Ok(ProcessorRetryRequest {
        graph_id: failed.graph_id.clone(),
        node_id: failed.node_id.clone(),
        expected_revision,
        expected_catalog_revision: harness.catalog_revision()?,
        expected_instance_id: failed.instance_id.clone(),
    })
}

fn send_retry(
    harness: &NativeHarness,
    request: ProcessorRetryRequest,
) -> Result<GraphApplyResponse, mixrelay_control_protocol::ProtocolError> {
    ControlBackend::retry_failed_processor(harness.controller.as_ref(), request)
}

fn expect_error(
    result: Result<impl Sized, mixrelay_control_protocol::ProtocolError>,
    expected: ErrorCode,
    phase: &str,
) -> Result<(), String> {
    match result {
        Err(error) if error.code == expected => Ok(()),
        Err(error) => Err(format!(
            "{phase}: expected {expected:?}, got {:?}",
            error.code
        )),
        Ok(_) => Err(format!("{phase}: unexpectedly accepted")),
    }
}

fn wait_new_worker(
    private_dir: &std::path::Path,
    previous: Heartbeat,
    timeout: Duration,
) -> Result<Heartbeat, String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(current) =
            worker::wait_heartbeat(private_dir, WorkerRole::Target, Duration::from_millis(100))
            && (current.pid != previous.pid || current.sequence < previous.sequence)
        {
            return Ok(current);
        }
    }
    Err("target worker heartbeat did not identify a replacement instance".into())
}

fn read_consumed_nonce(path: &std::path::Path, nonce: &str) -> Option<u32> {
    let contents = std::fs::read_to_string(path).ok()?;
    let mut fields = contents.split_whitespace();
    if fields.next()? != nonce {
        return None;
    }
    let pid = fields.next()?.parse().ok()?;
    fields.next().is_none().then_some(pid)
}

fn fault_nonce() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("native-recovery-{timestamp}")
}

fn configured_path(variable: &str) -> Result<PathBuf, String> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{variable} is not configured"))?
        .canonicalize()
        .map_err(|error| format!("{variable}: {error}"))
}

fn private_config_dir() -> Result<PathBuf, String> {
    std::env::var_os("MIXRELAY_CONFIG_DIR")
        .map(PathBuf::from)
        .ok_or("MIXRELAY_CONFIG_DIR is not configured".into())
}

fn verify_state(parameters: &Value, config_dir: &std::path::Path) -> Result<StateRef, String> {
    require_native_state_only(parameters)?;
    let reference = parameters
        .get("stateRef")
        .cloned()
        .ok_or("native checkpoint did not persist a state reference")?;
    let reference: StateRef =
        serde_json::from_value(reference).map_err(|error| error.to_string())?;
    if reference.byte_length() == 0 {
        return Err("native checkpoint state is empty".into());
    }
    let store = StateStore::new(config_dir.join("graphs.states"));
    store
        .verify(&reference)
        .map_err(|error| error.to_string())?;
    Ok(reference)
}

fn require_native_state_only(parameters: &Value) -> Result<(), String> {
    if !parameters
        .as_object()
        .is_some_and(|object| object.len() == 1 && object.contains_key("stateRef"))
    {
        return Err("checkpoint must contain only native state, without parameter overlays".into());
    }
    Ok(())
}

fn verify_heartbeat_stopped(
    private_dir: &std::path::Path,
    before_shutdown: Heartbeat,
) -> Result<(), String> {
    std::thread::sleep(Duration::from_millis(60));
    let first = worker::wait_heartbeat(private_dir, WorkerRole::Healthy, Duration::from_secs(1))?;
    std::thread::sleep(Duration::from_millis(60));
    let second = worker::wait_heartbeat(private_dir, WorkerRole::Healthy, Duration::from_secs(1))?;
    if first.pid != before_shutdown.pid
        || second.pid != before_shutdown.pid
        || first.sequence != second.sequence
    {
        return Err("owned healthy worker heartbeat continued after harness shutdown".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_proof_rejects_a_parameter_overlay_that_could_mask_failed_native_restore() {
        assert!(require_native_state_only(&serde_json::json!({"stateRef": {}})).is_ok());
        for parameters in [
            serde_json::json!({"stateRef": {}, "parameterValues": {"1":0.5}}),
            serde_json::json!({"parameterValues": {"1":0.5}}),
            Value::Null,
        ] {
            assert!(require_native_state_only(&parameters).is_err());
        }
    }
}
