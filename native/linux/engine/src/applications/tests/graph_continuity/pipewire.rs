//! Registry, metadata and process queries against the private lab `PipeWire` session.
#[cfg(feature = "native-lab")]
use super::*;

#[cfg(feature = "native-lab")]
pub(super) fn registry_snapshot() -> Vec<Value> {
    let output =
        mixrelay_linux_audio_io::bounded_pipewire_output("pw-dump", &[], Duration::from_secs(2))
            .expect("bounded private PipeWire registry read");
    assert!(output.status.success(), "private pw-dump failed");
    serde_json::Deserializer::from_slice(&output.stdout)
        .into_iter::<Vec<Value>>()
        .flat_map(|batch| batch.expect("valid private PipeWire registry batch"))
        .collect()
}

#[cfg(feature = "native-lab")]
pub(super) fn metadata_value(
    objects: &[Value],
    name: &str,
    subject: u32,
    key: &str,
) -> Option<String> {
    objects
        .iter()
        .find(|object| {
            object["type"] == "PipeWire:Interface:Metadata"
                && object["props"]["metadata.name"] == name
        })?
        .get("metadata")?
        .as_array()?
        .iter()
        .find(|entry| entry["subject"].as_u64() == Some(u64::from(subject)) && entry["key"] == key)?
        .get("value")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(feature = "native-lab")]
pub(super) fn default_metadata(
    objects: &[Value],
) -> std::collections::BTreeMap<(u32, String), Value> {
    let entries = objects
        .iter()
        .find(|object| {
            object["type"] == "PipeWire:Interface:Metadata"
                && object["props"]["metadata.name"] == "default"
        })
        .and_then(|object| object["metadata"].as_array())
        .expect("private WirePlumber default metadata");
    entries
        .iter()
        .map(|entry| {
            (
                (
                    u32::try_from(entry["subject"].as_u64().expect("metadata subject")).unwrap(),
                    entry["key"].as_str().expect("metadata key").to_owned(),
                ),
                entry.clone(),
            )
        })
        .collect()
}

pub(super) fn private_target_args<'a>(source: &'a str, target: Option<&'a str>) -> Vec<&'a str> {
    let mut args = vec!["-n", "default", source, "target.object"];
    if let Some(target) = target {
        args.extend([target, "Spa:String"]);
    } else {
        args.insert(0, "-d");
    }
    args
}

#[test]
pub(super) fn private_target_restore_deletes_only_the_exact_private_key() {
    assert_eq!(
        private_target_args("41", Some("io.mixrelay.fixture.policy-reroute-sink")),
        [
            "-n",
            "default",
            "41",
            "target.object",
            "io.mixrelay.fixture.policy-reroute-sink",
            "Spa:String"
        ]
    );
    assert_eq!(
        private_target_args("41", None),
        ["-d", "-n", "default", "41", "target.object"]
    );
}

#[cfg(feature = "native-lab")]
pub(super) fn update_private_target(source: u32, target: Option<&str>) {
    let source = source.to_string();
    let args = private_target_args(&source, target);
    let output = mixrelay_linux_audio_io::bounded_pipewire_output(
        "pw-metadata",
        &args,
        Duration::from_secs(2),
    )
    .expect("bounded private playback target change");
    assert!(
        output.status.success(),
        "private playback target change failed"
    );
}

#[cfg(feature = "native-lab")]
pub(super) fn wait_for_ack(
    backend: &crate::backend::LinuxVirtualEndpointBackend,
    exchange: &mixrelay_render::ProcessorExchange,
    source_serial: &str,
    source_id: u32,
    endpoint_id: u32,
) -> String {
    let deadline = Instant::now() + SETUP_TIMEOUT;
    let lease_key = format!("lease.{source_serial}");
    let ack_key = format!("ack.{source_serial}");
    loop {
        backend.poll_source_mute(exchange);
        let objects = registry_snapshot();
        let lease = metadata_value(&objects, "mixrelay-source-mute", endpoint_id, &lease_key);
        let ack = metadata_value(&objects, "mixrelay-source-mute", endpoint_id, &ack_key);
        if let (Some(lease), Some(ack)) = (lease, ack)
            && lease == ack
            && managed_links(&objects, source_id, endpoint_id).is_empty()
        {
            return lease;
        }
        assert!(
            Instant::now() < deadline,
            "exact WP ACK/leased-link removal absent"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(feature = "native-lab")]
pub(super) fn wait_for_lease_clear(
    backend: &crate::backend::LinuxVirtualEndpointBackend,
    exchange: &mixrelay_render::ProcessorExchange,
    source_serial: &str,
    endpoint_id: u32,
) {
    let deadline = Instant::now() + SETUP_TIMEOUT;
    let lease_key = format!("lease.{source_serial}");
    let ack_key = format!("ack.{source_serial}");
    loop {
        backend.poll_source_mute(exchange);
        let objects = registry_snapshot();
        if metadata_value(&objects, "mixrelay-source-mute", endpoint_id, &lease_key).is_none()
            && metadata_value(&objects, "mixrelay-source-mute", endpoint_id, &ack_key).is_none()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "exact WP lease/ACK did not clear"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(feature = "native-lab")]
pub(super) fn managed_links(objects: &[Value], source: u32, sink: u32) -> Vec<u32> {
    objects
        .iter()
        .filter(|object| {
            object["type"] == "PipeWire:Interface:Link"
                && object["info"]["output-node-id"].as_u64() == Some(u64::from(source))
                && object["info"]["input-node-id"].as_u64() == Some(u64::from(sink))
        })
        .map(|link| {
            let client_id = link["info"]["props"]["client.id"]
                .as_u64()
                .expect("managed link has server-bound client ID");
            let client = objects
                .iter()
                .find(|object| object["id"].as_u64() == Some(client_id))
                .expect("managed link has live client");
            assert_eq!(client["type"], "PipeWire:Interface:Client");
            // SAFETY: geteuid reads only this process credential, with no pointer access.
            let uid = unsafe { libc::geteuid() };
            assert_eq!(
                client["info"]["props"]["pipewire.sec.uid"].as_u64(),
                Some(u64::from(uid))
            );
            let pid = u32::try_from(
                client["info"]["props"]["pipewire.sec.pid"]
                    .as_u64()
                    .expect("server client PID"),
            )
            .unwrap();
            let executable = mixrelay_linux_audio_io::process_identity::process_identity(pid)
                .and_then(|identity| identity.executable_path)
                .expect("live link client executable");
            assert_eq!(
                Path::new(&executable)
                    .file_name()
                    .and_then(|name| name.to_str()),
                Some("wireplumber")
            );
            u32::try_from(link["id"].as_u64().expect("link ID")).unwrap()
        })
        .collect()
}

#[cfg(feature = "native-lab")]
pub(super) fn wait_for_managed_links(source: u32, sink: u32, expected: usize) -> Vec<u32> {
    let deadline = Instant::now() + SETUP_TIMEOUT;
    loop {
        let registry = registry_snapshot();
        let links = managed_links(&registry, source, sink);
        if links.len() == expected {
            return links;
        }
        if Instant::now() >= deadline {
            let fixture = std::env::var_os("MIXRELAY_LAB_APPLICATION_FIXTURE")
                .map(PathBuf::from)
                .expect("explicit owned fixture path");
            let evidence = fixture.parent().unwrap().join(format!(
                "managed-route-timeout-{source}-{sink}-{expected}.json"
            ));
            let output = File::create_new(&evidence).expect("exclusive failed-route evidence");
            serde_json::to_writer(output, &registry).expect("save failed-route registry");
            panic!(
                "WP-managed route count did not become {expected}; found {links:?}; evidence={}",
                evidence.display()
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(feature = "native-lab")]
pub(super) fn wait_driver_ready(
    driver: &mixrelay_linux_pipewire::GraphDriver<LinuxGraphProcess<'_>>,
    status: &mixrelay_engine_core::session::AdapterRuntimeStatus,
) {
    let deadline = Instant::now() + SETUP_TIMEOUT;
    while !driver.ready() {
        assert!(
            !status.stop_requested(),
            "engine graph stopped before ready"
        );
        assert!(
            Instant::now() < deadline,
            "Linux GraphDriver did not become ready"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "native-lab")]
pub(super) fn process_start_time(process_id: u32) -> u64 {
    let stat = std::fs::read_to_string(format!("/proc/{process_id}/stat"))
        .expect("read exact engine-process identity");
    stat.rsplit_once(')')
        .and_then(|(_, rest)| {
            rest.split_whitespace()
                .nth(19)
                .and_then(|value| value.parse::<u64>().ok())
        })
        .expect("parse exact engine-process start time")
}

#[cfg(feature = "native-lab")]
pub(super) fn wait_for_owned_target(
    process_id: u32,
    app_id: &str,
    node_name: &str,
    media_class: &str,
) -> mixrelay_linux_audio_io::PipeWireTarget {
    let deadline = Instant::now() + SETUP_TIMEOUT;
    loop {
        let inventory = mixrelay_linux_audio_io::targets().expect("private PipeWire inventory");
        let candidates: Vec<_> = inventory
            .into_iter()
            .filter(|target| {
                target.process_id == Some(process_id)
                    && target.application_id.as_deref() == Some(app_id)
                    && target.node_name.as_deref() == Some(node_name)
                    && target.media_class == media_class
            })
            .collect();
        assert!(candidates.len() <= 1, "ambiguous test-owned node identity");
        if let Some(target) = candidates.into_iter().next() {
            assert!(!target.serial.is_empty(), "owned node lacks object serial");
            return target;
        }
        assert!(
            Instant::now() < deadline,
            "owned PipeWire node did not appear"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "native-lab")]
pub(super) fn verified_sink_node_name(ready: &Value, sink_process_id: u32) -> String {
    assert_eq!(ready["event"], "ready");
    assert_eq!(ready["routeMode"], "graph");
    assert_eq!(ready["applicationId"], SINK_APP_ID);
    assert_eq!(
        ready["processId"].as_u64(),
        Some(u64::from(sink_process_id))
    );
    let serial = ready["nodeSerial"]
        .as_str()
        .filter(|value| !value.is_empty())
        .expect("verified private Audio/Sink serial")
        .to_owned();
    let target = wait_for_owned_target(sink_process_id, SINK_APP_ID, SINK_NODE_NAME, "Audio/Sink");
    assert_eq!(ready["nodeId"].as_u64(), Some(u64::from(target.object_id)));
    assert_eq!(target.serial, serial);
    target
        .node_name
        .expect("verified sink has a persistent node name")
}

#[cfg(feature = "native-lab")]
pub(super) fn verify_route_ready(route: &Value, process_id: u32, start_time: u64) {
    assert_eq!(route["event"], "routeReady");
    assert_eq!(route["processId"].as_u64(), Some(u64::from(process_id)));
    assert_eq!(route["processStartTime"].as_u64(), Some(start_time));
    assert!(route["sourceNodeSerial"].as_str().is_some());
    assert!(route["sinkNodeSerial"].as_str().is_some());
    let links = route["linkIds"]
        .as_array()
        .expect("exact graph route links");
    assert_eq!(links.len(), 2);
    assert!(links.iter().all(|id| id.as_u64().is_some_and(|id| id != 0)));
    assert_ne!(links[0], links[1]);
}
