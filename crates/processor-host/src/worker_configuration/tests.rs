use super::*;
use mixrelay_test_support::FixtureDir;

#[test]
fn worker_configuration_restores_large_state_from_a_small_reference() {
    let fixture = FixtureDir::new("processor-state-startup").unwrap();
    let store = StateStore::new(fixture.join("states"));
    let bytes = vec![0xa5; 9 * 1024 * 1024];
    let reference = store.put(&bytes).unwrap();
    let parameters = serde_json::json!({"stateRef":reference,"parameterValues":{"7":0.4}});
    let path = fixture.join("worker.json");
    let ports = mixrelay_audio_graph::ProviderPorts::canonical_stereo("fixture-stereo");
    write_worker_config(&path, 48_000, &parameters, Some(&store), &ports).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() < 1024);
    let prepared = read_worker_configuration(path.to_str().unwrap()).unwrap();
    assert_eq!(prepared.sample_rate.to_bits(), 48_000.0_f64.to_bits());
    assert_eq!(prepared.ports, ports);
    assert_eq!(prepared.initial.native.unwrap(), bytes);
    assert_eq!(prepared.initial.parameters, parameters);
}

#[test]
fn worker_configuration_is_private_and_never_overwrites_an_existing_owner() {
    let directory = FixtureDir::new("worker-config").unwrap();
    let path = directory.join("worker.json");
    let ports = mixrelay_audio_graph::ProviderPorts::canonical_stereo("stereo");
    write_worker_config(&path, 48_000, &serde_json::json!({}), None, &ports).unwrap();
    let original = std::fs::read(&path).unwrap();

    assert!(write_worker_config(&path, 96_000, &Value::Null, None, &ports).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn worker_configuration_rejects_valid_json_that_exceeds_the_startup_limit() {
    let directory = FixtureDir::new("worker-config").unwrap();
    let path = directory.join("worker.json");
    let ports = mixrelay_audio_graph::ProviderPorts::canonical_stereo("stereo");
    let parameters = Value::String("a".repeat(usize::try_from(MAX_CONFIGURATION_BYTES).unwrap()));
    write_worker_config(&path, 48_000, &parameters, None, &ports).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > MAX_CONFIGURATION_BYTES);

    assert!(read_worker_configuration(path.to_str().unwrap()).is_err());
}

#[test]
fn invalid_startup_rate_or_relative_state_root_cannot_reach_native_restoration() {
    let directory = FixtureDir::new("worker-config").unwrap();
    let path = directory.join("worker.json");
    let ports = mixrelay_audio_graph::ProviderPorts::canonical_stereo("stereo");
    for (sample_rate, state_store) in [(0, None), (48_000, Some(PathBuf::from("relative")))] {
        let config = WorkerConfiguration {
            sample_rate,
            ports: ports.clone(),
            parameters: Value::Null,
            state_store,
        };
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();

        assert!(read_worker_configuration(path.to_str().unwrap()).is_err());
    }
}
