use std::fs;

use mixrelay_test_support::FixtureDir;

use super::*;
use mixrelay_driver_control::{DriverCommand, DriverManager, test_backend::FakeBackend};

#[test]
fn malformed_store_is_preserved_without_native_apply() {
    let fixture = FixtureDir::new("driver-invalid-store").unwrap();
    let path = fixture.join("drivers.json");
    fs::write(&path, b"not JSON").unwrap();
    let (backend, observed) = FakeBackend::ready();
    assert!(
        DriverManager::open(
            backend,
            FileSettingsStore::new(path.clone()),
            mixrelay_driver_control::test_runtime::runtime()
        )
        .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"not JSON");
    assert_eq!(observed.lock().applies, 0);
}

#[test]
fn stopped_state_is_saved_with_retained_devices_in_the_private_settings_file() {
    let fixture = FixtureDir::new("driver-stopped-store").unwrap();
    let path = fixture.join("drivers.json");
    let (backend, _) = FakeBackend::ready();
    let mut manager = DriverManager::open(
        backend,
        FileSettingsStore::new(path.clone()),
        mixrelay_driver_control::test_runtime::runtime(),
    )
    .unwrap();
    let revision = manager.snapshot().unwrap().revision;
    manager
        .mutate(DriverCommand::Deactivate {
            expected_revision: revision,
        })
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(json["active"], false);
    assert!(!json["devices"].as_array().unwrap().is_empty());
    let entries: Vec<_> = fs::read_dir(fixture.path()).unwrap().collect();
    assert_eq!(entries.len(), 1, "the run state has no sibling file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    drop(manager);
    let (backend, observed) = FakeBackend::ready();
    let mut restarted = DriverManager::open(
        backend,
        FileSettingsStore::new(path),
        mixrelay_driver_control::test_runtime::runtime(),
    )
    .unwrap();
    assert!(restarted.snapshot().unwrap().settings.devices.is_empty());
    assert!(observed.lock().settings.devices.is_empty());
}

#[test]
fn absent_and_explicitly_empty_settings_remain_distinct() {
    let fixture = FixtureDir::new("driver-file-empty").unwrap();
    let store = FileSettingsStore::new(fixture.join("nested/drivers.json"));
    assert_eq!(store.load().unwrap(), None);
    let saved = StoredSettings {
        revision: 7,
        active: false,
        devices: vec![],
    };
    store.save(&saved).unwrap();
    assert_eq!(store.load().unwrap(), Some(saved));
}

#[test]
fn missing_fields_and_unknown_fields_are_not_rewritten() {
    let fixture = FixtureDir::new("driver-file-schema").unwrap();
    let path = fixture.join("drivers.json");
    let store = FileSettingsStore::new(&path);
    for bytes in [
        br#"{"revision":7,"devices":[]}"#.as_slice(),
        br#"{"revision":7,"active":true,"devices":[],"extra":1}"#,
    ] {
        fs::write(&path, bytes).unwrap();
        assert!(matches!(store.load(), Err(DriverError::Json(_))));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn reads_accept_one_mib_and_reject_larger_files_and_io_errors() {
    let fixture = FixtureDir::new("driver-file-limit").unwrap();
    let path = fixture.join("drivers.json");
    let store = FileSettingsStore::new(&path);
    fs::create_dir(&path).unwrap();
    assert!(matches!(store.load(), Err(DriverError::Io(_))));
    fs::remove_dir(&path).unwrap();
    let mut bytes = br#"{"revision":7,"active":true,"devices":[]}"#.to_vec();
    bytes.resize(JSON_LIMIT, b' ');
    fs::write(&path, &bytes).unwrap();
    assert!(store.load().unwrap().unwrap().devices.is_empty());
    bytes.push(b' ');
    fs::write(&path, &bytes).unwrap();
    assert!(matches!(store.load(), Err(DriverError::Invalid(_))));
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn oversized_save_preserves_the_committed_configuration() {
    let fixture = FixtureDir::new("driver-file-save-limit").unwrap();
    let path = fixture.join("drivers.json");
    let (backend, _) = FakeBackend::ready();
    let manager = DriverManager::open(
        backend,
        FileSettingsStore::new(&path),
        mixrelay_driver_control::test_runtime::runtime(),
    )
    .unwrap();
    let store = FileSettingsStore::new(&path);
    let saved = store.load().unwrap().unwrap();
    let mut oversized = saved.clone();
    oversized.devices[0].spec.display_name = "x".repeat(JSON_LIMIT);
    assert!(matches!(
        store.save(&oversized),
        Err(DriverError::Invalid(_))
    ));
    assert_eq!(store.load().unwrap(), Some(saved));
    assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 1);
    drop(manager);
}

#[test]
fn failed_publish_preserves_the_destination_and_removes_its_temporary_file() {
    let fixture = FixtureDir::new("driver-file-rename").unwrap();
    let path = fixture.join("drivers.json");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("existing"), b"keep").unwrap();
    let store = FileSettingsStore::new(&path);
    assert!(matches!(
        store.save(&StoredSettings {
            revision: 1,
            active: false,
            devices: vec![]
        }),
        Err(DriverError::Io(_))
    ));
    assert_eq!(fs::read(path.join("existing")).unwrap(), b"keep");
    assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 1);
}

#[test]
fn parent_creation_failure_preserves_the_existing_file() {
    let fixture = FixtureDir::new("driver-file-parent").unwrap();
    let parent = fixture.join("settings");
    fs::write(&parent, b"keep").unwrap();
    let store = FileSettingsStore::new(parent.join("drivers.json"));
    assert!(matches!(
        store.save(&StoredSettings {
            revision: 1,
            active: false,
            devices: vec![]
        }),
        Err(DriverError::Io(_))
    ));
    assert_eq!(fs::read(&parent).unwrap(), b"keep");
    assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 1);
}
