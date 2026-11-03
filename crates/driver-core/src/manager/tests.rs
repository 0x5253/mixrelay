use mixrelay_audio_io::{NativeSampleFormat, VirtualEndpointId};

use super::*;
use crate::{test_backend::FakeBackend, test_settings::MemorySettings};

#[test]
fn maintenance_preserves_pause_and_recovers_only_failed_retained_devices() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let mut second = default_device();
    second.spec.id = VirtualEndpointId::from("healthy-other-device");
    second.spec.display_name = "Healthy other device".into();
    manager
        .mutate(DriverCommand::Create {
            expected_revision: manager.retained.revision,
            spec: second.spec,
        })
        .unwrap();
    let settings = manager.settings.clone();
    let inventory = manager.retained.devices.clone();
    let saved = store.load().unwrap();
    let applies = observed.lock().applies;
    let readbacks = observed.lock().readbacks;
    observed.lock().state = DriverState::Unavailable; // Non-streaming, with no terminal failure.
    assert!(!manager.maintain().recovery.unwrap());
    assert_eq!(observed.lock().applies, applies);
    assert_eq!(observed.lock().readbacks, readbacks);
    assert_eq!(manager.settings, settings);
    assert_eq!(manager.retained.devices, inventory);
    let failed = inventory[0].spec.id.as_str().to_owned();
    observed.lock().failed_devices.insert(failed.clone());
    assert!(manager.maintain().recovery.unwrap());
    assert_eq!(observed.lock().recovered_devices, [failed]);
    assert_eq!(manager.settings, settings);
    assert_eq!(manager.retained.devices, inventory);
    assert_eq!(store.load().unwrap(), saved);
    assert!(!manager.maintain().recovery.unwrap());
    assert_eq!(observed.lock().applies, applies + 1);
    assert_eq!(observed.lock().readbacks, readbacks + 1);
}

#[test]
fn failed_recovery_retries_same_intent_without_saving_or_reviving_removed_devices() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let desired = manager.retained.devices.clone();
    let saved = store.load().unwrap();
    let failed = desired[0].spec.id.as_str().to_owned();
    {
        let mut native = observed.lock();
        native.failed_devices.insert(failed.clone());
        native.state = DriverState::Unavailable;
        native.fail_apply = true;
    }
    assert!(manager.maintain().recovery.is_err());
    assert_eq!(manager.retained.devices, desired);
    assert_eq!(store.load().unwrap(), saved);
    observed.lock().fail_apply = false;
    assert!(manager.maintain().recovery.unwrap());
    manager
        .mutate(DriverCommand::Remove {
            expected_revision: manager.retained.revision,
            endpoint_id: desired[0].spec.id.clone(),
        })
        .unwrap();
    let applies = observed.lock().applies;
    observed.lock().failed_devices.insert(failed);
    assert!(!manager.maintain().recovery.unwrap());
    assert!(manager.retained.devices.is_empty());
    assert!(observed.lock().settings.devices.is_empty());
    assert_eq!(observed.lock().applies, applies);
}

#[test]
fn invalid_geometry_rate_duplicates_and_missing_ids_have_no_side_effects() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let snapshot = manager.snapshot().unwrap();
    let before = store.load().unwrap();
    for channels in [0, 65] {
        let mut spec = default_device().spec;
        spec.render.as_mut().unwrap().channels = channels;
        assert!(
            manager
                .mutate(DriverCommand::Configure {
                    expected_revision: snapshot.revision,
                    spec
                })
                .is_err()
        );
    }
    let mut spec = default_device().spec;
    spec.supported_sample_rates = vec![12_345];
    spec.default_sample_rate = 12_345;
    assert!(
        manager
            .mutate(DriverCommand::Configure {
                expected_revision: snapshot.revision,
                spec
            })
            .is_err()
    );
    assert!(
        manager
            .mutate(DriverCommand::Create {
                expected_revision: snapshot.revision,
                spec: default_device().spec
            })
            .is_err()
    );
    assert!(matches!(
        manager.mutate(DriverCommand::Remove {
            expected_revision: snapshot.revision,
            endpoint_id: VirtualEndpointId::from("absent")
        }),
        Err(DriverError::NotFound(_))
    ));
    assert_eq!(observed.lock().applies, 1);
    assert_eq!(store.load().unwrap(), before);
}

#[test]
fn native_apply_failure_preserves_active_inventory_and_store() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let before = manager.snapshot().unwrap();
    let bytes = store.load().unwrap();
    observed.lock().fail_apply = true;
    assert!(
        manager
            .mutate(DriverCommand::Remove {
                expected_revision: before.revision,
                endpoint_id: before.settings.devices[0].spec.id.clone()
            })
            .is_err()
    );
    assert_eq!(manager.snapshot().unwrap(), before);
    assert_eq!(store.load().unwrap(), bytes);
}

#[test]
fn next_change_after_a_pending_apply_starts_from_where_the_hal_settled() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let before = manager.snapshot().unwrap();
    let configure = |manager: &mut DriverManager, revision, rate| {
        let mut spec = before.settings.devices[0].spec.clone();
        spec.default_sample_rate = rate;
        manager.mutate(DriverCommand::Configure {
            expected_revision: revision,
            spec,
        })
    };
    observed.lock().pending_apply = true;
    assert!(matches!(
        configure(&mut manager, before.revision, 96_000),
        Err(DriverError::ConfigurationPending)
    ));
    assert_eq!(manager.snapshot().unwrap(), before);
    // The HAL drains and lands the forward rate after the service reported pending.
    {
        let mut observation = observed.lock();
        observation.pending_apply = false;
        let device = &mut observation.settings.devices[0];
        device.spec.default_sample_rate = 96_000;
        device.observed_sample_rate = Some(96_000);
        for formats in [
            &mut device.native_formats.render,
            &mut device.native_formats.capture,
        ] {
            formats.current.as_mut().unwrap().sample_rate = 96_000;
        }
    }
    // Readback, not the intent kept at the pending error, becomes the current state.
    let settled = manager.snapshot().unwrap();
    assert!(settled.revision > before.revision);
    assert_eq!(
        settled.settings.devices[0].observed_sample_rate,
        Some(96_000)
    );
    assert_eq!(
        store.load().unwrap().unwrap().devices[0]
            .spec
            .default_sample_rate,
        96_000
    );
    assert!(matches!(
        configure(&mut manager, before.revision, 48_000),
        Err(DriverError::RevisionMismatch { .. })
    ));
    let after = configure(&mut manager, settled.revision, 48_000).unwrap();
    assert_eq!(after.settings.devices[0].observed_sample_rate, Some(48_000));
}

#[test]
fn external_native_change_invalidates_revision_and_survives_name_change() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let before = manager.snapshot().unwrap();
    {
        let mut observation = observed.lock();
        let device = &mut observation.settings.devices[0];
        device.spec.default_sample_rate = 44_100;
        device.observed_sample_rate = Some(44_100);
        for formats in [
            &mut device.native_formats.render,
            &mut device.native_formats.capture,
        ] {
            formats.current.as_mut().unwrap().sample_rate = 44_100;
            for format in &mut formats.supported {
                format.sample_rate = 44_100;
            }
        }
    }
    assert!(matches!(
        manager.mutate(DriverCommand::Remove {
            expected_revision: before.revision,
            endpoint_id: before.settings.devices[0].spec.id.clone()
        }),
        Err(DriverError::RevisionMismatch { .. })
    ));
    let actual = manager.snapshot().unwrap();
    assert_eq!(
        store.load().unwrap().unwrap().devices[0]
            .spec
            .default_sample_rate,
        44_100
    );
    let mut spec = actual.settings.devices[0].spec.clone();
    spec.display_name = "User device".into();
    let after = manager
        .mutate(DriverCommand::Configure {
            expected_revision: actual.revision,
            spec,
        })
        .unwrap();
    assert_eq!(after.settings.devices[0].observed_sample_rate, Some(44_100));
    assert_eq!(
        store.load().unwrap().unwrap().devices[0]
            .spec
            .default_sample_rate,
        44_100
    );
}

#[test]
fn set_format_requires_exact_supported_tuple_and_preserves_other_pcm() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let snapshot = manager.snapshot().unwrap();
    let id = snapshot.settings.devices[0].spec.id.clone();
    let current = snapshot.settings.devices[0]
        .native_formats
        .render
        .current
        .unwrap();
    let unsupported = NativeStreamFormat {
        channels: 3,
        ..current
    };
    assert!(
        manager
            .mutate(DriverCommand::SetFormat {
                expected_revision: snapshot.revision,
                endpoint_id: id.clone(),
                direction: NativeDirection::Render,
                format: unsupported
            })
            .is_err()
    );
    assert_eq!(observed.lock().applies, 1);
    let integer = NativeStreamFormat {
        sample_format: NativeSampleFormat::Signed16,
        ..current
    };
    let after = manager
        .mutate(DriverCommand::SetFormat {
            expected_revision: snapshot.revision,
            endpoint_id: id,
            direction: NativeDirection::Render,
            format: integer,
        })
        .unwrap();
    assert_eq!(
        after.settings.devices[0].native_formats.render.current,
        Some(integer)
    );
    assert_eq!(
        after.settings.devices[0].native_formats.capture.current,
        Some(current)
    );
}

#[test]
fn shared_sample_format_applies_duplex_once_and_preserves_rate_and_channels() {
    let store = MemorySettings::default();
    let mut saved = default_device();
    saved.spec.default_sample_rate = 44_100;
    saved.spec.capture.as_mut().unwrap().channels = 4;
    saved.spec.capture.as_mut().unwrap().channel_layout = ChannelLayout::Discrete;
    saved.render_format.as_mut().unwrap().sample_rate = 44_100;
    saved.capture_format.as_mut().unwrap().sample_rate = 44_100;
    saved.capture_format.as_mut().unwrap().channels = 4;
    store
        .save(&StoredSettings {
            revision: 7,
            active: true,
            devices: vec![saved],
        })
        .unwrap();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let before = manager.snapshot().unwrap();
    let device = &before.settings.devices[0];
    let applies = observed.lock().applies;

    let after = manager
        .mutate(DriverCommand::SetSampleFormat {
            expected_revision: before.revision,
            endpoint_id: device.spec.id.clone(),
            sample_format: NativeSampleFormat::Signed16,
        })
        .unwrap();

    assert_eq!(after.revision, before.revision + 1);
    assert_eq!(observed.lock().applies, applies + 1);
    assert_eq!(after.settings.devices[0].spec, device.spec);
    for (previous, current) in [
        (
            device.native_formats.render.current,
            after.settings.devices[0].native_formats.render.current,
        ),
        (
            device.native_formats.capture.current,
            after.settings.devices[0].native_formats.capture.current,
        ),
    ] {
        assert_eq!(
            current,
            Some(NativeStreamFormat {
                sample_format: NativeSampleFormat::Signed16,
                ..previous.unwrap()
            })
        );
    }
    let persisted = store.load().unwrap().unwrap();
    assert_eq!(persisted.revision, after.revision);
    assert_eq!(persisted.devices, manager.retained.devices);
}

#[test]
fn shared_sample_format_rejects_unsupported_capture_tuple_without_partial_apply() {
    for change_channels in [false, true] {
        let store = MemorySettings::default();
        let (backend, observed) = FakeBackend::ready();
        let mut manager =
            DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
        {
            let mut observation = observed.lock();
            let supported = &mut observation.settings.devices[0]
                .native_formats
                .capture
                .supported;
            let integer = supported
                .iter_mut()
                .find(|format| format.sample_format == NativeSampleFormat::Signed16)
                .unwrap();
            if change_channels {
                integer.channels = 4;
            } else {
                integer.sample_rate = 44_100;
            }
        }
        let before = manager.snapshot().unwrap();
        let bytes = store.load().unwrap();
        let applies = observed.lock().applies;

        assert!(matches!(
            manager.mutate(DriverCommand::SetSampleFormat {
                expected_revision: before.revision,
                endpoint_id: before.settings.devices[0].spec.id.clone(),
                sample_format: NativeSampleFormat::Signed16,
            }),
            Err(DriverError::Invalid(_))
        ));
        assert_eq!(manager.snapshot().unwrap(), before);
        assert_eq!(observed.lock().applies, applies);
        assert_eq!(store.load().unwrap(), bytes);
    }
}

#[test]
fn shared_sample_format_does_not_create_an_absent_direction() {
    for direction in [NativeDirection::Render, NativeDirection::Capture] {
        let store = MemorySettings::default();
        let mut saved = default_device();
        match direction {
            NativeDirection::Render => {
                saved.spec.capture = None;
                saved.capture_format = None;
            }
            NativeDirection::Capture => {
                saved.spec.render = None;
                saved.render_format = None;
            }
        }
        store
            .save(&StoredSettings {
                revision: 7,
                active: true,
                devices: vec![saved.clone()],
            })
            .unwrap();
        let (backend, observed) = FakeBackend::ready();
        let mut manager =
            DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
        let before = manager.snapshot().unwrap();
        let applies = observed.lock().applies;

        let after = manager
            .mutate(DriverCommand::SetSampleFormat {
                expected_revision: before.revision,
                endpoint_id: saved.spec.id.clone(),
                sample_format: NativeSampleFormat::Signed16,
            })
            .unwrap();

        assert_eq!(after.revision, before.revision + 1);
        assert_eq!(observed.lock().applies, applies + 1);
        assert_eq!(after.settings.devices[0].spec, saved.spec);
        for (stream, native) in [
            (
                saved.spec.render,
                &after.settings.devices[0].native_formats.render,
            ),
            (
                saved.spec.capture,
                &after.settings.devices[0].native_formats.capture,
            ),
        ] {
            if stream.is_some() {
                assert_eq!(
                    native.current.unwrap().sample_format,
                    NativeSampleFormat::Signed16
                );
            } else {
                assert!(native.current.is_none());
                assert!(native.supported.is_empty());
            }
        }
    }
}

#[test]
fn shared_sample_format_requires_current_observation_for_every_present_direction() {
    for direction in [NativeDirection::Render, NativeDirection::Capture] {
        let store = MemorySettings::default();
        let (backend, observed) = FakeBackend::ready();
        let mut manager =
            DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
        {
            let mut observation = observed.lock();
            let formats = &mut observation.settings.devices[0].native_formats;
            match direction {
                NativeDirection::Render => formats.render.current = None,
                NativeDirection::Capture => formats.capture.current = None,
            }
        }
        let before = manager.snapshot().unwrap();
        let bytes = store.load().unwrap();
        let applies = observed.lock().applies;

        assert!(matches!(
            manager.mutate(DriverCommand::SetSampleFormat {
                expected_revision: before.revision,
                endpoint_id: before.settings.devices[0].spec.id.clone(),
                sample_format: NativeSampleFormat::Signed16,
            }),
            Err(DriverError::Invalid(_))
        ));
        assert_eq!(manager.snapshot().unwrap(), before);
        assert_eq!(observed.lock().applies, applies);
        assert_eq!(store.load().unwrap(), bytes);
    }
}

#[test]
fn shared_sample_format_rejects_stale_revision_and_missing_identity() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let before = manager.snapshot().unwrap();
    let bytes = store.load().unwrap();
    let applies = observed.lock().applies;

    assert!(matches!(
        manager.mutate(DriverCommand::SetSampleFormat {
            expected_revision: before.revision - 1,
            endpoint_id: before.settings.devices[0].spec.id.clone(),
            sample_format: NativeSampleFormat::Signed16,
        }),
        Err(DriverError::RevisionMismatch { .. })
    ));
    assert!(matches!(
        manager.mutate(DriverCommand::SetSampleFormat {
            expected_revision: before.revision,
            endpoint_id: VirtualEndpointId::from("absent"),
            sample_format: NativeSampleFormat::Signed16,
        }),
        Err(DriverError::NotFound(_))
    ));
    assert_eq!(manager.snapshot().unwrap(), before);
    assert_eq!(observed.lock().applies, applies);
    assert_eq!(store.load().unwrap(), bytes);
}

#[test]
fn shared_sample_format_native_failure_preserves_both_directions_and_store() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let before = manager.snapshot().unwrap();
    let bytes = store.load().unwrap();
    let applies = observed.lock().applies;
    observed.lock().fail_apply = true;

    assert!(matches!(
        manager.mutate(DriverCommand::SetSampleFormat {
            expected_revision: before.revision,
            endpoint_id: before.settings.devices[0].spec.id.clone(),
            sample_format: NativeSampleFormat::Signed16,
        }),
        Err(DriverError::Backend(_))
    ));
    assert_eq!(manager.snapshot().unwrap(), before);
    assert_eq!(observed.lock().applies, applies + 1);
    assert_eq!(store.load().unwrap(), bytes);
}

#[test]
fn absent_native_formats_are_not_invented_during_snapshot_or_save() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    observed.lock().settings.devices[0].native_formats.render =
        mixrelay_audio_io::NativeStreamFormats::default();
    let snapshot = manager.snapshot().unwrap();
    assert!(
        snapshot.settings.devices[0]
            .native_formats
            .render
            .current
            .is_none()
    );
    assert!(manager.retained.devices[0].render_format.is_none());
    assert!(
        store.load().unwrap().unwrap().devices[0]
            .render_format
            .is_none()
    );
}

/// Applies a latency command at a synthetic instant, so elapsed test time cannot expire it.
fn latency_at(
    manager: &mut DriverManager,
    command: crate::LatencyCommand,
    now: mixrelay_time::Instant,
) -> crate::LatencyReply {
    manager
        .latency
        .command(command, now, manager.backend.as_mut())
        .unwrap()
}

fn latency_publication(
    lease: &crate::EngineLatencyLease,
    endpoint: &VirtualEndpointId,
    revision: u64,
    frames: u32,
) -> crate::LatencyCommand {
    crate::LatencyCommand::Publish {
        lease: lease.clone(),
        revision,
        destinations: vec![crate::RuntimeLatency {
            endpoint_id: endpoint.clone(),
            rate: 48_000,
            engine_lease: lease.clone(),
            applied_graph_revision: revision,
            input_latency_frames: Some(frames),
        }],
    }
}

#[test]
fn maintenance_clears_latency_from_an_engine_that_stopped_publishing() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let mut manager =
        DriverManager::open(backend, store.clone(), crate::test_runtime::runtime()).unwrap();
    let last_latency = || {
        observed
            .lock()
            .latencies
            .last()
            .unwrap()
            .input_latency_frames
    };
    let now = mixrelay_time::Instant::from_origin(std::time::Duration::from_secs(123));
    let reply = latency_at(
        &mut manager,
        crate::LatencyCommand::Claim {
            incarnation: "engine".into(),
            process_id: 42,
        },
        now,
    );
    let lease = reply.lease.unwrap();
    let endpoint = manager.retained.devices[0].spec.id.clone();
    let publish = latency_publication(&lease, &endpoint, 1, 256);
    assert!(latency_at(&mut manager, publish, now).accepted);
    manager.maintain_at(now).recovery.unwrap();
    assert_eq!(last_latency(), Some(256));
    manager
        .maintain_at(now.saturating_add(std::time::Duration::from_secs(4)))
        .recovery
        .unwrap();
    assert_eq!(last_latency(), None);
    // Engine death never removes the independent Driver inventory.
    let retained = manager.snapshot().unwrap().settings;
    assert_eq!(retained.devices.len(), 1);
    let recovered_at = now.saturating_add(std::time::Duration::from_secs(4));
    let replacement = latency_at(
        &mut manager,
        crate::LatencyCommand::Claim {
            incarnation: "replacement".into(),
            process_id: 43,
        },
        recovered_at,
    )
    .lease
    .unwrap();
    assert_ne!(replacement, lease);
    // A delayed report from the dead engine cannot revive its old measurement.
    assert!(
        !latency_at(
            &mut manager,
            latency_publication(&lease, &endpoint, 100, 999),
            recovered_at
        )
        .accepted
    );
    assert_eq!(last_latency(), None);
    assert!(
        latency_at(
            &mut manager,
            latency_publication(&replacement, &endpoint, 1, 128),
            recovered_at
        )
        .accepted
    );
    assert_eq!(last_latency(), Some(128));
    assert_eq!(manager.snapshot().unwrap().settings, retained);
}

mod persistence;
