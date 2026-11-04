use super::*;
use crate::source_mute::lease::tests::lease;
use mixrelay_linux_audio_io::process_identity::process_identity;

#[test]
fn driver_sink_authority_is_pinned_to_canonical_name_serial_and_process() {
    let driver_owner = process_identity(std::process::id()).unwrap();
    // SAFETY: geteuid reads this test process's credential without pointer access.
    let uid = unsafe { libc::geteuid() };
    let name = "io.mixrelay.endpoint.fixture.render";
    let objects = serde_json::json!([
        {"id":9,"type":"PipeWire:Interface:Client","info":{"props":{
            "object.serial":91,"pipewire.sec.pid":driver_owner.process_id,"pipewire.sec.uid":uid}}},
        {"id":12,"type":"PipeWire:Interface:Node","info":{"props":{
            "object.serial":121,"client.id":9,"node.name":name,"media.class":"Audio/Sink",
            "application.id":"io.mixrelay.engine","application.process.id":999_999}}}
    ]);
    let registry =
        Registry::parse(&serde_json::to_vec(&objects).unwrap(), Host::current()).unwrap();
    assert_eq!(
        registry.endpoint(12, name, &driver_owner).as_deref(),
        Some("121")
    );
    assert_eq!(registry.client_serial_for_node(12).as_deref(), Some("91"));
    assert!(
        registry
            .endpoint(12, "another-device.render", &driver_owner)
            .is_none()
    );
    let mut stale_owner = driver_owner.clone();
    stale_owner.start_time += 1;
    assert!(registry.endpoint(12, name, &stale_owner).is_none());
    let mut other_process = driver_owner;
    other_process.process_id = 999_999;
    assert!(registry.endpoint(12, name, &other_process).is_none());
    let mut recycled = objects;
    recycled[1]["info"]["props"]["object.serial"] = serde_json::json!(122);
    let registry =
        Registry::parse(&serde_json::to_vec(&recycled).unwrap(), Host::current()).unwrap();
    assert!(!registry.verified_node(12, "121", "Audio/Sink", &stale_owner));
}

#[test]
fn server_client_identity_not_node_claim_controls_lease_target() {
    let pid = std::process::id();
    let uid = unsafe { libc::geteuid() };
    let bytes = serde_json::to_vec(&serde_json::json!([
        {"id": 9, "type": "PipeWire:Interface:Client", "info": {"props": {
            "object.serial": 91, "pipewire.sec.pid": pid, "pipewire.sec.uid": uid
        }}},
        {"id": 12, "type": "PipeWire:Interface:Node", "info": {"props": {
            "object.serial": 121, "client.id": 9,
            "media.class": "Stream/Output/Audio", "application.process.id": 999_999
        }}}
    ]))
    .unwrap();
    let registry = Registry::parse(&bytes, Host::current()).unwrap();
    let process = process_identity(pid).unwrap();
    assert_eq!(registry.client_serial_for_node(12).as_deref(), Some("91"));
    assert!(registry.verified_node(12, "121", "Stream/Output/Audio", &process));
    assert!(!registry.verified_node(12, "122", "Stream/Output/Audio", &process));
    assert!(!registry.verified_node(12, "121", "Audio/Sink", &process));
    let mut recycled = process.clone();
    recycled.start_time += 1;
    assert!(!registry.verified_node(12, "121", "Stream/Output/Audio", &recycled));
}

#[test]
fn pulse_broker_requires_actual_broker_executable_and_matching_app_claim() {
    let selected = ProcessIdentity {
        process_id: 163_453,
        parent_id: 1,
        start_time: 91,
        executable_path: Some("/usr/lib/firefox-esr/firefox-esr".into()),
    };
    let mut broker = ProcessIdentity {
        process_id: 654,
        parent_id: 1,
        start_time: 12,
        executable_path: Some("/usr/bin/pipewire-pulse".into()),
    };
    let node = serde_json::json!({
        "application.process.id": 163_453,
        "application.process.binary": "firefox-esr"
    });
    let mut client = serde_json::json!({
        "application.process.id": 163_453,
        "application.process.binary": "firefox-esr",
        "client.api": "pipewire-pulse", "config.name": "pipewire-pulse.conf",
        "pulse.server.type": "unix"
    });
    assert!(pulse_claim_matches(&broker, &selected, &node, &client));
    broker.executable_path = Some("/usr/bin/unrelated".into());
    assert!(!pulse_claim_matches(&broker, &selected, &node, &client));
    broker.executable_path = Some("/usr/bin/pipewire-pulse".into());
    client["application.process.id"] = serde_json::json!(4000);
    assert!(!pulse_claim_matches(&broker, &selected, &node, &client));
}

#[test]
fn routing_target_reads_policy_metadata_and_rejects_unverified_owner() {
    let metadata = serde_json::json!({"metadata":[
        {"subject":12,"key":"target.object","value":"owned.sink"},
        {"subject":13,"key":"target.object","value":"other.sink"}
    ]});
    assert_eq!(
        routing_target_value(&metadata, 12),
        Some(ObservedTarget::Target("owned.sink".into()))
    );
    assert_eq!(
        routing_target_value(&metadata, 14),
        Some(ObservedTarget::Unset)
    );
    let mut malformed = metadata.clone();
    malformed["metadata"][0]
        .as_object_mut()
        .unwrap()
        .remove("value");
    assert_eq!(routing_target_value(&malformed, 12), None);
    let mut changed = metadata.clone();
    changed["metadata"][0]["value"] = serde_json::json!("other.sink");
    assert_eq!(
        routing_target_value(&changed, 12),
        Some(ObservedTarget::Target("other.sink".into()))
    );

    let pid = std::process::id();
    let uid = unsafe { libc::geteuid() };
    let registry = Registry::parse(
        &serde_json::to_vec(&serde_json::json!([
            {"id":9,"type":"PipeWire:Interface:Client","info":{"props":{
                "object.serial":91,"pipewire.sec.pid":pid,"pipewire.sec.uid":uid}}},
            {"id":30,"type":"PipeWire:Interface:Metadata","props":{
                "metadata.name":"default","client.id":9},"metadata":metadata["metadata"]}
        ]))
        .unwrap(),
        Host::current(),
    )
    .unwrap();
    // The test process is not WirePlumber even if it claims a default metadata name.
    assert_eq!(registry.routing_target(12), None);
}

#[test]
fn metadata_and_link_readback_require_exact_subject_and_owner() {
    let bytes = serde_json::to_vec(&serde_json::json!([
        {"id": 30, "type": "PipeWire:Interface:Metadata", "props": {
            "metadata.name": "mixrelay-source-mute", "client.id": 9
        }, "metadata": [
            {"subject": 12, "key": "ack.121", "value": "55:121:1:2:nonce"},
            {"subject": 13, "key": "ack.121", "value": "wrong"}
        ]},
        {"id": 40, "type": "PipeWire:Interface:Link", "info": {
            "output-node-id": 12, "input-node-id": 55,
            "props": {"client.id": 9}
        }}
    ]))
    .unwrap();
    let registry = Registry::parse(&bytes, Host::current()).unwrap();
    assert_eq!(
        registry.metadata_value(12, "ack.121"),
        Some("55:121:1:2:nonce")
    );
    assert_eq!(registry.metadata_value(13, "ack.121"), Some("wrong"));
    assert_eq!(registry.metadata_value(12, "ack.122"), None);
    let process = process_identity(std::process::id()).unwrap();
    let lease = lease(&process);
    assert_eq!(registry.links(&lease), vec![(40, false)]);
}

#[test]
fn appended_registry_removal_does_not_retain_old_link() {
    let bytes = b"[{\"id\":40,\"type\":\"PipeWire:Interface:Link\",\"info\":{\"output-node-id\":12,\"input-node-id\":55}}][{\"id\":40,\"info\":null}]";
    let registry = Registry::parse(bytes, Host::current()).unwrap();
    assert_eq!(registry.object_ids().count(), 0);
}
