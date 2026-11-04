use super::*;
use crate::source_mute::registry::{Host, ObservedTarget};
use mixrelay_linux_audio_io::process_identity::process_identity;
use serde_json::Value;

/// A lease from endpoint 55 (serial 55) to source 12 (serial 121), all owned by `process`.
pub(in crate::source_mute) fn lease(process: &ProcessIdentity) -> Lease {
    Lease {
        key: LeaseKey {
            endpoint_serial: "55".into(),
            source_serial: "121".into(),
        },
        endpoint_id: 55,
        source_id: 12,
        endpoint_process: process.clone(),
        source_process: process.clone(),
        source_owner: process.clone(),
        endpoint_client_serial: "1".into(),
        source_client_serial: "2".into(),
        capture_healthy: Arc::new(AtomicBool::new(true)),
        capture_fresh: Arc::new(AtomicBool::new(true)),
        nonce: "nonce".into(),
        references: 1,
        other_sink_serials: BTreeSet::new(),
        routing_target: None,
        source_target_hint: None,
    }
}

#[test]
fn pending_apply_keeps_old_lease_until_owner_death_or_reroute() {
    let process = process_identity(std::process::id()).unwrap();
    let mut lease = lease(&process);
    lease.nonce = "old-active-plan".into();
    let key = lease.key.clone();
    let mut state = LeaseState::default();
    state.leases.insert(key.clone(), lease);
    let no_links = Registry::parse(b"[]", Host::current()).unwrap();
    assert!(
        state
            .invalid_keys(|old| {
                same_process(&old.source_process, &process) && no_links.links(old).is_empty()
            })
            .is_empty()
    );
    assert!(state.leases.contains_key(&key));

    let current = &state.leases[&key];
    assert!(current.capture_ready());
    current
        .capture_fresh
        .store(false, std::sync::atomic::Ordering::Release);
    assert_eq!(state.invalid_keys(Lease::capture_ready), vec![key.clone()]);
    current
        .capture_fresh
        .store(true, std::sync::atomic::Ordering::Release);
    current
        .capture_healthy
        .store(false, std::sync::atomic::Ordering::Release);
    assert!(!current.capture_ready());
    current
        .capture_healthy
        .store(true, std::sync::atomic::Ordering::Release);

    let mut recycled = process.clone();
    recycled.start_time += 1;
    assert_eq!(
        state.invalid_keys(|old| {
            same_process(&old.source_process, &recycled) && no_links.links(old).is_empty()
        }),
        vec![key.clone()]
    );
    let reroute = Registry::parse(
        br#"[{"id":40,"type":"PipeWire:Interface:Link","info":{"output-node-id":12,"input-node-id":55}}]"#,
        Host::current(),
    )
    .unwrap();
    assert_eq!(
        state.invalid_keys(|old| {
            same_process(&old.source_process, &process) && reroute.links(old).is_empty()
        }),
        vec![key]
    );
}

fn route_fixture() -> (Lease, Vec<Value>) {
    let process = process_identity(std::process::id()).unwrap();
    let lease = Lease {
        routing_target: Some("owned.sink".into()),
        source_target_hint: Some("owned.sink".into()),
        ..lease(&process)
    };
    let objects = vec![
        serde_json::json!({"id":70,"type":"PipeWire:Interface:Node","info":{"props":{
            "object.serial":700,"media.class":"Audio/Sink"}}}),
        serde_json::json!({"id":90,"type":"PipeWire:Interface:Node","info":{"props":{
            "object.serial":900,"media.class":"Stream/Input/Audio"}}}),
        serde_json::json!({"id":20,"type":"PipeWire:Interface:Link","info":{
            "output-node-id":12,"input-node-id":55}}),
        serde_json::json!({"id":21,"type":"PipeWire:Interface:Link","info":{
            "output-node-id":12,"input-node-id":70}}),
        serde_json::json!({"id":22,"type":"PipeWire:Interface:Link","info":{
            "output-node-id":12,"input-node-id":70}}),
        serde_json::json!({"id":23,"type":"PipeWire:Interface:Link","info":{
            "output-node-id":12,"input-node-id":90}}),
        serde_json::json!({"id":12,"type":"PipeWire:Interface:Node","info":{"props":{
            "object.serial":121,"media.class":"Stream/Output/Audio",
            "target.object":"owned.sink"}}}),
    ];
    (lease, objects)
}

#[test]
fn installed_route_baseline_ignores_taps_and_stereo_relinks_but_detects_new_sinks() {
    let (mut lease, mut objects) = route_fixture();
    let snapshot = |objects: &[Value]| {
        Registry::parse(&serde_json::to_vec(objects).unwrap(), Host::current()).unwrap()
    };
    let baseline = snapshot(&objects).other_sink_serials(&lease).unwrap();
    assert_eq!(baseline, BTreeSet::from(["700".to_owned()]));
    lease.other_sink_serials = baseline;
    assert_eq!(
        snapshot(&objects).source_target_hint(lease.source_id),
        Some(ObservedTarget::Target("owned.sink".into()))
    );
    assert!(lease.route_matches(
        &snapshot(&objects).other_sink_serials(&lease).unwrap(),
        Some("owned.sink")
    ));

    // A stereo Link ID can be recreated without changing its sink owner.
    objects[4]["id"] = serde_json::json!(24);
    assert!(lease.route_matches(
        &snapshot(&objects).other_sink_serials(&lease).unwrap(),
        Some("owned.sink")
    ));
    // Removing an existing other sink does not interfere with that user's route.
    objects.retain(|object| !matches!(object["id"].as_u64(), Some(21 | 24)));
    assert!(lease.route_matches(
        &snapshot(&objects).other_sink_serials(&lease).unwrap(),
        Some("owned.sink")
    ));
    assert!(!lease.route_matches(
        &snapshot(&objects).other_sink_serials(&lease).unwrap(),
        Some("other.sink")
    ));
    objects.push(
        serde_json::json!({"id":80,"type":"PipeWire:Interface:Node","info":{"props":{
        "object.serial":800,"media.class":"Audio/Sink"}}}),
    );
    objects.push(
        serde_json::json!({"id":25,"type":"PipeWire:Interface:Link","info":{
        "output-node-id":12,"input-node-id":80}}),
    );
    assert!(!lease.route_matches(
        &snapshot(&objects).other_sink_serials(&lease).unwrap(),
        Some("owned.sink")
    ));
    // Reusing a transient node ID with a new server serial is a new route.
    objects.retain(|object| !matches!(object["id"].as_u64(), Some(25 | 80)));
    objects[0]["info"]["props"]["object.serial"] = serde_json::json!(701);
    objects.push(
        serde_json::json!({"id":26,"type":"PipeWire:Interface:Link","info":{
        "output-node-id":12,"input-node-id":70}}),
    );
    assert!(!lease.route_matches(
        &snapshot(&objects).other_sink_serials(&lease).unwrap(),
        Some("owned.sink")
    ));
}

#[test]
fn changed_source_hint_revokes_even_when_other_sink_serials_and_policy_target_match() {
    let (mut lease, mut objects) = route_fixture();
    let snapshot = |objects: &[Value]| {
        Registry::parse(&serde_json::to_vec(objects).unwrap(), Host::current()).unwrap()
    };
    lease.other_sink_serials = snapshot(&objects).other_sink_serials(&lease).unwrap();
    let source = objects
        .iter()
        .position(|object| object["id"] == 12)
        .unwrap();
    objects[source]["info"]["props"]["target.object"] = serde_json::json!("old-other.sink");
    assert!(lease.route_matches(
        &snapshot(&objects).other_sink_serials(&lease).unwrap(),
        Some("owned.sink")
    ));
    assert_ne!(
        snapshot(&objects).source_target_hint(lease.source_id),
        Some(ObservedTarget::Target("owned.sink".into()))
    );
    objects[source]["info"]["props"]["target.object"] = Value::Null;
    assert_eq!(
        snapshot(&objects).source_target_hint(lease.source_id),
        Some(ObservedTarget::Unset)
    );
    objects[source]["info"]["props"]["target.object"] = serde_json::json!(42);
    assert_eq!(snapshot(&objects).source_target_hint(lease.source_id), None);
}
