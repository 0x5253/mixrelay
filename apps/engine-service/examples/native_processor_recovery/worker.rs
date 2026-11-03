use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::dispatch::PRIVATE_ENV;

const ROLE_FILE: &str = "worker-roles.json";
const HEARTBEAT_PERIOD: Duration = Duration::from_millis(20);
const TARGET_FAULT: &str = "target-fault.request";
const TARGET_FAULT_CONSUMED: &str = "target-fault.consumed";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum WorkerRole {
    Target,
    Healthy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Heartbeat {
    pub(super) pid: u32,
    pub(super) sequence: u64,
}

pub(super) fn wait_heartbeat(
    private_dir: &Path,
    role: WorkerRole,
    timeout: Duration,
) -> Result<Heartbeat, String> {
    let path = private_dir.join(format!("{}.heartbeat", role.name()));
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(contents) = std::fs::read_to_string(&path)
            && let Some(heartbeat) = parse_heartbeat(&contents)
        {
            return Ok(heartbeat);
        }
        thread::sleep(Duration::from_millis(2));
    }
    Err(format!("{} worker heartbeat was not readable", role.name()))
}

fn parse_heartbeat(contents: &str) -> Option<Heartbeat> {
    let mut fields = contents.split_whitespace();
    let pid = fields.next()?;
    let sequence = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    Some(Heartbeat {
        pid: pid.parse().ok()?,
        sequence: sequence.parse().ok()?,
    })
}

pub(super) fn add_role(
    private_dir: &Path,
    parameters: &Value,
    role: WorkerRole,
) -> Result<(), String> {
    let path = private_dir.join(ROLE_FILE);
    let mut roles: BTreeMap<String, WorkerRole> = if path.exists() {
        serde_json::from_slice(&std::fs::read(&path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?
    } else {
        BTreeMap::new()
    };
    let key = fingerprint(parameters)?;
    if roles
        .insert(key, role)
        .is_some_and(|previous| previous != role)
    {
        return Err("worker parameter identity is ambiguous".into());
    }
    let bytes = serde_json::to_vec(&roles).map_err(|error| error.to_string())?;
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

pub(super) fn fingerprint(parameters: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(parameters).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(super) struct WorkerInstrumentation {
    cancel: Sender<()>,
    watcher: Option<JoinHandle<()>>,
}

impl WorkerInstrumentation {
    pub(super) fn start(arguments: &[String]) -> Result<Self, String> {
        let [_, _, config_path] = arguments else {
            return Err("worker instrumentation needs the private config path".into());
        };
        let private_dir = private_directory()?;
        let config: Value =
            serde_json::from_slice(&std::fs::read(config_path).map_err(|error| error.to_string())?)
                .map_err(|error| format!("invalid private worker config: {error}"))?;
        let parameters = config
            .get("parameters")
            .ok_or("worker config is missing parameters")?;
        let role_file = private_dir.join(ROLE_FILE);
        let roles: BTreeMap<String, WorkerRole> =
            serde_json::from_slice(&std::fs::read(role_file).map_err(|error| error.to_string())?)
                .map_err(|error| format!("invalid worker role registry: {error}"))?;
        let role = roles
            .get(&fingerprint(parameters)?)
            .copied()
            .ok_or("worker parameters have no qualification role")?;
        let pid = std::process::id();
        let heartbeat = private_dir.join(format!("{}.heartbeat", role.name()));
        std::fs::write(&heartbeat, format!("{pid} 0")).map_err(|error| error.to_string())?;

        let (cancel, receiver) = mpsc::channel();
        let watcher = thread::Builder::new()
            .name(format!("native-recovery-{}", role.name()))
            .spawn(move || monitor_worker(&private_dir, role, pid, &heartbeat, &receiver))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            cancel,
            watcher: Some(watcher),
        })
    }
}

impl Drop for WorkerInstrumentation {
    fn drop(&mut self) {
        let _ = self.cancel.send(());
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}

fn monitor_worker(
    private_dir: &Path,
    role: WorkerRole,
    pid: u32,
    heartbeat: &Path,
    cancel: &Receiver<()>,
) {
    let marker = private_dir.join(TARGET_FAULT);
    let consumed = private_dir.join(TARGET_FAULT_CONSUMED);
    let mut sequence = 0_u64;
    loop {
        match cancel.recv_timeout(HEARTBEAT_PERIOD) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if role == WorkerRole::Target && marker.exists() {
            consume_fault_marker(&marker, &consumed, pid);
            std::process::exit(77);
        }
        sequence = sequence.saturating_add(1);
        if std::fs::write(heartbeat, format!("{pid} {sequence}")).is_err() {
            return;
        }
    }
}

fn consume_fault_marker(marker: &Path, consumed: &Path, pid: u32) {
    let token = std::fs::read_to_string(marker).unwrap_or_default();
    if std::fs::remove_file(marker).is_err()
        || std::fs::write(consumed, format!("{} {pid}", token.trim())).is_err()
    {
        std::process::exit(78);
    }
}

fn private_directory() -> Result<PathBuf, String> {
    let value = std::env::var_os(PRIVATE_ENV)
        .map(PathBuf::from)
        .ok_or("qualification private directory is not configured")?;
    if !value.is_absolute() {
        return Err("qualification private directory must be absolute".into());
    }
    value.canonicalize().map_err(|error| error.to_string())
}

impl WorkerRole {
    const fn name(self) -> &'static str {
        match self {
            Self::Target => "target",
            Self::Healthy => "healthy",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_parser_rejects_partial_and_extra_fields() {
        assert_eq!(
            parse_heartbeat("123 7"),
            Some(Heartbeat {
                pid: 123,
                sequence: 7
            })
        );
        for value in ["", "123", "abc 7", "123 -1", "123 7 extra"] {
            assert_eq!(parse_heartbeat(value), None);
        }
    }

    #[test]
    fn role_fingerprint_distinguishes_target_and_healthy_presets() {
        let target = serde_json::json!({"parameterValues":{"1":0.5,"2":0.25}});
        let healthy = serde_json::json!({"parameterValues":{"1":0.75,"2":0.5}});
        let reordered: Value =
            serde_json::from_str(r#"{"parameterValues":{"2":0.25,"1":0.5}}"#).unwrap();
        assert_ne!(
            fingerprint(&target).unwrap(),
            fingerprint(&healthy).unwrap()
        );
        assert_eq!(
            fingerprint(&target).unwrap(),
            fingerprint(&reordered).unwrap()
        );
    }
}
