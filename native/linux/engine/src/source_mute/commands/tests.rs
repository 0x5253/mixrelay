//! The lease protocol against scripted `pw-dump` snapshots on a manual clock.
use super::*;
use crate::source_mute::lease::LeaseKey;
use mixrelay_linux_audio_io::process_identity::ProcessIdentity;
use mixrelay_time::{Instant, ManualClock};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::{Arc, Mutex, atomic::AtomicBool},
};

const WIREPLUMBER: u32 = 10;
const DRIVER: u32 = 20;
const PLAYER: u32 = 30;
const EUID: u32 = 1000;

fn identify(pid: u32) -> Option<ProcessIdentity> {
    let executable = match pid {
        WIREPLUMBER => "/usr/bin/wireplumber",
        DRIVER => "/usr/libexec/mixrelay-driver",
        PLAYER => "/usr/bin/player",
        _ => return None,
    };
    Some(ProcessIdentity {
        process_id: pid,
        parent_id: 1,
        start_time: u64::from(pid) * 100,
        executable_path: Some(executable.into()),
    })
}

const HOST: Host = Host {
    euid: EUID,
    identify,
};

/// Serves snapshots in order, repeating the last, and records every command it runs.
struct ScriptedTools {
    snapshots: Mutex<VecDeque<Vec<Value>>>,
    commands: Arc<Mutex<Vec<String>>>,
}

impl PipeWireTools for ScriptedTools {
    fn read_registry(&self, _timeout: Duration) -> Result<Registry, String> {
        let mut snapshots = self.snapshots.lock().unwrap();
        let snapshot = if snapshots.len() > 1 {
            snapshots.pop_front().unwrap()
        } else {
            snapshots.front().cloned().unwrap()
        };
        Registry::parse(&serde_json::to_vec(&snapshot).unwrap(), HOST)
    }

    fn run(&self, program: &str, args: &[String], _timeout: Duration) -> Result<(), String> {
        self.commands
            .lock()
            .unwrap()
            .push(format!("{program} {}", args.join(" ")));
        Ok(())
    }
}

struct Harness {
    session: PipeWireSession,
    clock: ManualClock,
    commands: Arc<Mutex<Vec<String>>>,
}

fn harness(snapshots: Vec<Vec<Value>>) -> Harness {
    let clock = ManualClock::default();
    let commands = Arc::default();
    let tools = ScriptedTools {
        snapshots: Mutex::new(snapshots.into()),
        commands: Arc::clone(&commands),
    };
    Harness {
        session: PipeWireSession::new(tools, clock.clone()),
        clock,
        commands,
    }
}

impl Harness {
    fn commands(&self) -> Vec<String> {
        self.commands.lock().unwrap().clone()
    }

    fn elapsed(&self) -> Duration {
        self.clock
            .now()
            .duration_since(Instant::from_origin(Duration::ZERO))
    }
}

fn client(id: u32, pid: u32, extra: &Value) -> Value {
    let mut props =
        json!({"object.serial": id * 10, "pipewire.sec.pid": pid, "pipewire.sec.uid": EUID});
    props
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    json!({"id": id, "type": "PipeWire:Interface:Client", "info": {"props": props}})
}

fn metadata(id: u32, name: &str, entries: &Value) -> Value {
    json!({"id": id, "type": "PipeWire:Interface:Metadata",
        "props": {"metadata.name": name, "client.id": 9}, "metadata": entries})
}

fn link(id: u32, client: u32) -> Value {
    json!({"id": id, "type": "PipeWire:Interface:Link", "info": {
        "output-node-id": 12, "input-node-id": 55, "props": {"client.id": client}}})
}

/// `WirePlumber` 0.5 (client 9), the Driver sink 55 (client 21) and the player's
/// stream 12 (client 31), plus the source-mute metadata entries and any extra objects.
fn registry(mute_entries: &Value, extra: &[Value]) -> Vec<Value> {
    let mut objects = vec![
        client(9, WIREPLUMBER, &json!({"application.version": "0.5.8"})),
        client(21, DRIVER, &json!({})),
        client(31, PLAYER, &json!({})),
        json!({"id": 55, "type": "PipeWire:Interface:Node", "info": {"props": {
            "object.serial": 55, "client.id": 21, "media.class": "Audio/Sink"}}}),
        json!({"id": 12, "type": "PipeWire:Interface:Node", "info": {"props": {
            "object.serial": 121, "client.id": 31, "media.class": "Stream/Output/Audio"}}}),
        metadata(70, "default", &json!([])),
        metadata(71, METADATA_NAME, mute_entries),
    ];
    objects.extend_from_slice(extra);
    objects
}

fn lease() -> Lease {
    Lease {
        key: LeaseKey {
            endpoint_serial: "55".into(),
            source_serial: "121".into(),
        },
        endpoint_id: 55,
        source_id: 12,
        endpoint_process: identify(DRIVER).unwrap(),
        source_process: identify(PLAYER).unwrap(),
        source_owner: identify(PLAYER).unwrap(),
        endpoint_client_serial: "210".into(),
        source_client_serial: "310".into(),
        capture_healthy: Arc::new(AtomicBool::new(true)),
        capture_fresh: Arc::new(AtomicBool::new(true)),
        nonce: "n1".into(),
        references: 1,
        other_sink_serials: BTreeSet::new(),
        routing_target: None,
        source_target_hint: None,
    }
}

fn lease_entry(lease: &Lease) -> Value {
    json!({"subject": 55, "key": "lease.121", "value": lease.value()})
}

fn ack_entry(value: &str) -> Value {
    json!({"subject": 55, "key": "ack.121", "value": value})
}

fn set_command(lease: &Lease) -> String {
    format!(
        "pw-metadata -n {METADATA_NAME} 55 lease.121 {}",
        lease.value()
    )
}

fn clear_command() -> String {
    format!("pw-metadata -d -n {METADATA_NAME} 55 lease.121")
}

#[test]
fn spent_lease_deadline_rejects_command_before_spawn() {
    let harness = harness(vec![registry(&json!([]), &[])]);
    let deadline = harness.session.deadline(Duration::from_millis(1));
    harness.clock.advance(Duration::from_millis(1));
    assert!(harness.session.remaining(deadline).is_err());
    assert!(
        harness
            .session
            .run_until("pw-cli", &["destroy".into(), "1".into()], deadline)
            .is_err()
    );
    assert!(harness.commands().is_empty());
}

#[test]
fn metadata_clear_deletes_only_the_exact_owned_lease_key() {
    let expected: [String; 5] = [
        "-d".into(),
        "-n".into(),
        METADATA_NAME.into(),
        "55".into(),
        "lease.121".into(),
    ];
    assert_eq!(metadata_clear_args(&lease()), expected);
}

#[test]
fn clear_waits_until_the_lease_key_is_gone() {
    let lease = lease();
    let present = registry(&json!([lease_entry(&lease)]), &[]);
    let harness = harness(vec![present.clone(), present, registry(&json!([]), &[])]);
    harness.session.metadata_clear(&lease).unwrap();
    assert_eq!(harness.commands(), [clear_command()]);
    assert_eq!(harness.elapsed(), 2 * POLL_INTERVAL);
}

#[test]
fn clear_gives_up_at_its_deadline_while_the_key_remains() {
    let lease = lease();
    let harness = harness(vec![registry(&json!([lease_entry(&lease)]), &[])]);
    let error = harness.session.metadata_clear(&lease).unwrap_err();
    assert!(error.contains("deadline expired"), "{error}");
    assert_eq!(harness.elapsed(), CLEAR_TIMEOUT);
}

#[test]
fn acknowledged_install_removes_only_wireplumber_links_to_the_endpoint() {
    let mut lease = lease();
    let acknowledged = json!([lease_entry(&lease), ack_entry(&lease.value())]);
    let harness = harness(vec![
        registry(&json!([]), &[link(40, 9)]),
        registry(&json!([lease_entry(&lease)]), &[link(40, 9)]),
        registry(&acknowledged, &[link(40, 9)]),
        registry(&acknowledged, &[]),
    ]);
    harness.session.install(&mut lease).unwrap();
    assert_eq!(
        harness.commands(),
        [set_command(&lease), "pw-cli destroy 40".to_owned()]
    );
    assert_eq!(lease.routing_target, None);
    assert_eq!(lease.source_target_hint, None);
    assert!(lease.other_sink_serials.is_empty());
}

#[test]
fn install_never_removes_a_link_wireplumber_did_not_create() {
    let mut lease = lease();
    let acknowledged = json!([lease_entry(&lease), ack_entry(&lease.value())]);
    let harness = harness(vec![
        registry(&json!([]), &[link(40, 31)]),
        registry(&acknowledged, &[link(40, 31)]),
        registry(&json!([]), &[link(40, 31)]),
    ]);
    assert!(harness.session.install(&mut lease).is_err());
    assert_eq!(harness.commands(), [set_command(&lease), clear_command()]);
}

#[test]
fn stale_acknowledgement_times_out_and_still_clears_the_lease() {
    let mut lease = lease();
    // WirePlumber keeps echoing an older lease value; the last snapshot repeats.
    let stale = json!([lease_entry(&lease), ack_entry("55:121:old-nonce")]);
    let harness = harness(vec![registry(&json!([]), &[]), registry(&stale, &[])]);
    let error = harness.session.install(&mut lease).unwrap_err();
    assert!(error.contains("deadline expired"), "{error}");
    assert_eq!(harness.commands(), [set_command(&lease), clear_command()]);
    assert_eq!(harness.elapsed(), INSTALL_TIMEOUT + CLEAR_TIMEOUT);
}
