use super::*;

#[test]
fn create_configure_remove_preserves_explicit_empty_after_restart() {
    let store = MemorySettings::default();
    store
        .save(&StoredSettings {
            revision: 0,
            active: true,
            devices: vec![],
        })
        .unwrap();
    let (backend, _) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let mut spec = default_device().spec;
    spec.id = VirtualEndpointId::from("507b3da3-265a-45c7-b8a2-84b6463b913c");
    spec.display_name = "My device".into();
    spec.render = Some(StreamSpec {
        channels: 64,
        channel_layout: ChannelLayout::Discrete,
    });
    spec.capture = None;
    let revision = manager.snapshot().unwrap().revision;
    let created = manager
        .mutate(DriverCommand::Create {
            expected_revision: revision,
            spec,
        })
        .unwrap();
    assert_eq!(
        created.settings.devices[0].spec.render.unwrap().channels,
        64
    );
    let mut spec = created.settings.devices[0].spec.clone();
    spec.display_name = "Renamed".into();
    spec.render.as_mut().unwrap().channels = 32;
    let configured = manager
        .mutate(DriverCommand::Configure {
            expected_revision: created.revision,
            spec,
        })
        .unwrap();
    assert_eq!(configured.settings.devices[0].spec.display_name, "Renamed");
    assert_eq!(
        configured.settings.devices[0]
            .native_formats
            .render
            .current
            .unwrap()
            .channels,
        32
    );
    manager
        .mutate(DriverCommand::Remove {
            expected_revision: configured.revision,
            endpoint_id: configured.settings.devices[0].spec.id.clone(),
        })
        .unwrap();
    drop(manager);
    let (backend, _) = FakeBackend::ready();
    let mut reopened =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    assert!(reopened.snapshot().unwrap().settings.devices.is_empty());
}

#[test]
fn absent_store_creates_fixed_default_but_existing_empty_stays_empty() {
    let store = MemorySettings::default();
    let (backend, _) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let snapshot = manager.snapshot().unwrap();
    let device = &snapshot.settings.devices[0];
    assert_eq!(
        device.spec.id.as_str(),
        "17e59d73-42c1-4f11-a8b4-6c2d913357e0"
    );
    assert_eq!(device.spec.display_name, "MixRelay IO");
    assert_eq!(
        device.native_formats.render.current.unwrap().sample_format,
        NativeSampleFormat::Float32
    );
    assert!(store.load().unwrap().is_some());
    store
        .save(&StoredSettings {
            revision: 9,
            active: true,
            devices: vec![],
        })
        .unwrap();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    assert!(manager.snapshot().unwrap().settings.devices.is_empty());
    assert!(observed.lock().settings.devices.is_empty());
}

#[test]
fn load_failure_prevents_native_apply_and_default_save() {
    let store = MemorySettings::default();
    store.0.lock().fail_load = true;
    let (backend, observed) = FakeBackend::ready();
    assert!(matches!(
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()),
        Err(DriverError::Io(_))
    ));
    assert_eq!(observed.lock().applies, 0);
    assert_eq!(store.0.lock().writes, 0);
    assert!(store.0.lock().saved.is_none());
}

#[test]
fn unavailable_open_never_persists_defaults_or_accepts_mutations() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    observed.lock().state = DriverState::InstalledNotLoaded;
    observed.lock().fail_read = true;
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let snapshot = manager.snapshot().unwrap();
    assert_eq!(snapshot.state, DriverState::InstalledNotLoaded);
    assert!(!snapshot.settings.can_create);
    assert!(snapshot.settings.devices.is_empty());
    assert!(matches!(
        manager.mutate(DriverCommand::Create {
            expected_revision: snapshot.revision,
            spec: default_device().spec
        }),
        Err(DriverError::Unavailable(_))
    ));
    assert_eq!(observed.lock().applies, 0);
    assert!(store.load().unwrap().is_none());
}

#[test]
fn pending_default_waits_for_native_apply_before_exposing_or_saving_settings() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    observed.lock().state = DriverState::InstalledNotLoaded;
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    assert!(!manager.initialize().unwrap());
    let waiting = manager.snapshot().unwrap();
    assert_eq!(waiting.state, DriverState::InstalledNotLoaded);
    assert!(!waiting.settings.can_create);
    assert!(waiting.settings.devices.is_empty());
    assert_eq!(manager.retained.devices, vec![default_device()]);
    assert_eq!(observed.lock().applies, 0);
    assert!(store.load().unwrap().is_none());

    observed.lock().state = DriverState::Ready;
    let connected = manager.snapshot().unwrap();
    assert_eq!(connected.state, DriverState::Unavailable);
    assert!(!connected.settings.can_create);
    assert!(store.load().unwrap().is_none());
    assert!(manager.initialize().unwrap());
    assert!(manager.initialize().unwrap());
    let ready = manager.snapshot().unwrap();
    assert_eq!(ready.state, DriverState::Ready);
    assert_eq!(ready.settings.devices.len(), 1);
    assert_eq!(observed.lock().applies, 1);
    assert!(store.load().unwrap().is_some());
}

#[test]
fn pending_stored_configuration_and_explicit_empty_survive_late_readiness() {
    let mut saved = default_device();
    saved.spec.display_name = "Saved user device".into();
    saved.render_format.as_mut().unwrap().sample_format = NativeSampleFormat::Signed16;
    for devices in [vec![saved], vec![]] {
        let store = MemorySettings::default();
        store
            .save(&StoredSettings {
                revision: 7,
                active: true,
                devices: devices.clone(),
            })
            .unwrap();
        let bytes = store.load().unwrap();
        let (backend, observed) = FakeBackend::ready();
        observed.lock().state = DriverState::InstalledNotLoaded;
        let mut manager =
            DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
        assert_eq!(manager.retained.devices, devices);
        assert!(manager.snapshot().unwrap().settings.devices.is_empty());
        assert_eq!(store.load().unwrap(), bytes);
        assert_eq!(observed.lock().applies, 0);

        observed.lock().state = DriverState::Ready;
        assert!(manager.initialize().unwrap());
        assert_eq!(
            manager.snapshot().unwrap().settings.devices.len(),
            devices.len()
        );
        assert_eq!(store.load().unwrap().unwrap().devices, devices);
        assert_eq!(observed.lock().applies, 1);
    }
}

#[test]
fn failed_initial_apply_and_readback_preserve_pending_intent_and_store() {
    for fail_read in [false, true] {
        let store = MemorySettings::default();
        let desired = vec![default_device()];
        store
            .save(&StoredSettings {
                revision: 7,
                active: true,
                devices: desired.clone(),
            })
            .unwrap();
        let bytes = store.load().unwrap();
        let (backend, observed) = FakeBackend::ready();
        observed.lock().state = DriverState::InstalledNotLoaded;
        let mut manager =
            DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
        {
            let mut observation = observed.lock();
            observation.state = DriverState::Ready;
            observation.fail_apply = !fail_read;
            observation.fail_read = fail_read;
        }
        assert!(manager.initialize().is_err());
        assert!(manager.initial_inventory_pending);
        assert_eq!(manager.retained.devices, desired);
        let waiting = manager.snapshot().unwrap();
        assert_eq!(waiting.state, DriverState::Unavailable);
        assert!(!waiting.settings.can_create);
        assert!(waiting.settings.devices.is_empty());
        assert!(waiting.persistence_error.is_none());
        assert_eq!(store.load().unwrap(), bytes);
        {
            let mut observation = observed.lock();
            observation.fail_apply = false;
            observation.fail_read = false;
        }
        assert!(manager.initialize().unwrap());
        assert_eq!(manager.snapshot().unwrap().state, DriverState::Ready);
        assert_eq!(observed.lock().applies, 2);
    }
}

#[test]
fn persistence_failure_reports_committed_native_state_without_rollback() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let before = manager.snapshot().unwrap();
    let saved = store.load().unwrap();
    let writes = store.0.lock().writes;
    store.0.lock().fail_save = true;
    let after = manager
        .mutate(DriverCommand::Remove {
            expected_revision: before.revision,
            endpoint_id: before.settings.devices[0].spec.id.clone(),
        })
        .unwrap();
    assert!(after.settings.devices.is_empty());
    assert!(observed.lock().settings.devices.is_empty());
    assert!(after.persistence_error.is_some());
    assert!(after.revision > before.revision);
    assert_eq!(store.load().unwrap(), saved);
    assert_eq!(store.0.lock().writes, writes + 1);
}

#[test]
fn explicit_quit_stops_native_devices_and_restores_saved_geometry_after_restart() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let mut spec = manager.retained.devices[0].spec.clone();
    spec.display_name = "Retained studio device".into();
    spec.render.as_mut().unwrap().channels = 64;
    spec.render.as_mut().unwrap().channel_layout = ChannelLayout::Discrete;
    spec.capture.as_mut().unwrap().channels = 64;
    spec.capture.as_mut().unwrap().channel_layout = ChannelLayout::Discrete;
    let configured = manager
        .mutate(DriverCommand::Configure {
            expected_revision: manager.retained.revision,
            spec,
        })
        .unwrap();
    manager
        .mutate(DriverCommand::SetSampleFormat {
            expected_revision: configured.revision,
            endpoint_id: configured.settings.devices[0].spec.id.clone(),
            sample_format: NativeSampleFormat::Signed16,
        })
        .unwrap();
    let retained = manager.retained.devices.clone();
    let stopped = manager
        .mutate(DriverCommand::Deactivate {
            expected_revision: manager.retained.revision,
        })
        .unwrap();
    assert!(stopped.settings.devices.is_empty());
    assert!(observed.lock().settings.devices.is_empty());
    assert_eq!(manager.retained.devices, retained);
    let saved = store.load().unwrap().unwrap();
    assert!(!saved.active);
    assert_eq!(saved.devices, retained);
    observed
        .lock()
        .failed_devices
        .insert(retained[0].spec.id.as_str().into());
    let applies = observed.lock().applies;
    for _ in 0..3 {
        assert!(!manager.maintain().recovery.unwrap());
        assert!(manager.snapshot().unwrap().settings.devices.is_empty());
    }
    assert_eq!(observed.lock().applies, applies);
    assert_eq!(store.load().unwrap().unwrap().devices, retained);
    assert!(matches!(
        manager.mutate(DriverCommand::Configure {
            expected_revision: manager.retained.revision,
            spec: retained[0].spec.clone(),
        }),
        Err(DriverError::Unavailable(_))
    ));
    drop(manager);
    // A launchd/systemd restart retains the stop rather than reviving audio.
    let (backend, observed) = FakeBackend::ready();
    let mut restarted =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    assert!(restarted.snapshot().unwrap().settings.devices.is_empty());
    assert!(observed.lock().settings.devices.is_empty());
    assert_eq!(restarted.retained.devices, retained);
    let resumed = restarted
        .mutate(DriverCommand::Activate {
            expected_revision: restarted.retained.revision,
        })
        .unwrap();
    assert_eq!(observed_inventory(&resumed.settings), retained);
    assert!(store.load().unwrap().unwrap().active);
}

#[test]
fn failed_stop_preserves_running_devices_and_can_be_retried() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let running = manager.snapshot().unwrap();
    let saved = store.load().unwrap();
    observed.lock().fail_apply = true;
    assert!(
        manager
            .mutate(DriverCommand::Deactivate {
                expected_revision: running.revision,
            })
            .is_err()
    );
    assert!(manager.retained.active);
    assert_eq!(manager.snapshot().unwrap(), running);
    assert_eq!(store.load().unwrap(), saved);
    observed.lock().fail_apply = false;
    let stopped = manager
        .mutate(DriverCommand::Deactivate {
            expected_revision: running.revision,
        })
        .unwrap();
    assert!(stopped.settings.devices.is_empty());
    assert!(!manager.retained.active);
}

#[test]
fn stopped_state_save_failure_is_reported_and_idempotent_retry_saves_it() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let retained = manager.retained.devices.clone();
    store.0.lock().fail_save = true;
    assert!(
        manager
            .mutate(DriverCommand::Deactivate {
                expected_revision: manager.retained.revision,
            })
            .is_err()
    );
    assert!(!manager.retained.active);
    assert!(observed.lock().settings.devices.is_empty());
    assert!(!manager.maintain().recovery.unwrap());
    assert_eq!(manager.retained.devices, retained);
    store.0.lock().fail_save = false;
    manager
        .mutate(DriverCommand::Deactivate {
            expected_revision: manager.retained.revision,
        })
        .unwrap();
    let saved = store.load().unwrap().unwrap();
    assert!(!saved.active);
    assert_eq!(saved.devices, retained);
}

#[test]
fn saved_empty_inventory_stays_empty_across_stop_and_resume() {
    let store = MemorySettings::default();
    store
        .save(&StoredSettings {
            revision: 5,
            active: true,
            devices: vec![],
        })
        .unwrap();
    let (backend, _) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    manager
        .mutate(DriverCommand::Deactivate {
            expected_revision: manager.retained.revision,
        })
        .unwrap();
    manager
        .mutate(DriverCommand::Activate {
            expected_revision: manager.retained.revision,
        })
        .unwrap();
    assert!(manager.snapshot().unwrap().settings.devices.is_empty());
    assert!(store.load().unwrap().unwrap().devices.is_empty());
}

#[test]
fn stored_inventory_is_validated_before_native_apply() {
    let mut invalid_geometry = default_device();
    invalid_geometry.spec.render.as_mut().unwrap().channels = 0;
    let mut invalid_format = default_device();
    invalid_format.render_format.as_mut().unwrap().sample_rate = 96_000;
    for devices in [
        vec![invalid_geometry],
        vec![invalid_format],
        vec![default_device(), default_device()],
    ] {
        let store = MemorySettings::default();
        let saved = StoredSettings {
            revision: 7,
            active: true,
            devices,
        };
        store.save(&saved).unwrap();
        let (backend, observed) = FakeBackend::ready();
        assert!(matches!(
            DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()),
            Err(DriverError::Invalid(_))
        ));
        assert_eq!(observed.lock().applies, 0);
        assert_eq!(store.load().unwrap(), Some(saved));
        assert_eq!(store.0.lock().writes, 1);
    }
}

#[test]
fn initial_save_failure_retains_native_state_and_a_later_commit_clears_the_error() {
    let store = MemorySettings::default();
    store.0.lock().fail_save = true;
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let before = manager.snapshot().unwrap();
    assert!(before.persistence_error.is_some());
    assert_eq!(observed.lock().applies, 1);
    assert_eq!(before.settings.devices.len(), 1);
    assert!(store.load().unwrap().is_none());
    store.0.lock().fail_save = false;
    let after = manager
        .mutate(DriverCommand::Configure {
            expected_revision: before.revision,
            spec: before.settings.devices[0].spec.clone(),
        })
        .unwrap();
    assert!(after.persistence_error.is_none());
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.revision, after.revision);
    assert_eq!(saved.devices, observed_inventory(&after.settings));
}
