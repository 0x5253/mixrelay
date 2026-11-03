use super::*;
use mixrelay_macos_transport::{
    endpoint_transport::{
        EndpointTransportConfig, mr_endpoint_transport_attach, mr_endpoint_transport_config,
    },
    shared_audio_buffer::{
        mr_shared_audio_buffer_read, mr_shared_audio_buffer_stats,
        mr_shared_audio_buffer_try_begin_epoch, mr_shared_audio_buffer_write,
    },
};
use std::{
    ffi::c_void,
    fs::File,
    os::fd::{AsRawFd, BorrowedFd},
    ptr,
};

unsafe extern "C" {
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: i32,
        flags: i32,
        descriptor: i32,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> i32;
}

fn spec(id: u8) -> EndpointSpec {
    EndpointSpec {
        id: [id; 16],
        display_name: format!("Device {id}"),
        sample_rate: 96_000,
        capacity_frames: 64,
        render_channels: 7,
        capture_channels: 2,
    }
}
fn add(registry: &EndpointRegistry, id: u8) {
    let mut specs: Vec<_> = registry
        .state
        .lock()
        .active
        .values()
        .map(|mapping| mapping.spec().clone())
        .collect();
    specs.push(spec(id));
    registry
        .prepare_inventory(&specs)
        .unwrap()
        .commit()
        .unwrap();
}

fn pcm(handle: &CallbackEndpoint<'_>) {
    let input = [0.25; 14];
    let mut output = [0.0; 14];
    let before = unsafe { mr_shared_audio_buffer_stats(handle.render_buffer()) };
    // Reused endpoint mappings retain their monotonic timeline across inventory edits.
    // Append to that timeline; replaying frame zero is correctly rejected by the ring.
    let start = before.published_through_frame;
    assert!(unsafe {
        mr_shared_audio_buffer_write(
            handle.render_buffer(),
            before.epoch,
            start,
            input.as_ptr(),
            2,
        )
    });
    assert_eq!(
        unsafe {
            mr_shared_audio_buffer_read(handle.render_buffer(), start, output.as_mut_ptr(), 2)
        },
        2
    );
    assert_eq!(input.map(f32::to_bits), output.map(f32::to_bits));
    let after = unsafe { mr_shared_audio_buffer_stats(handle.render_buffer()) };
    assert_eq!(after.epoch, before.epoch);
    assert_eq!(after.published_through_frame, start + 2);
}

#[test]
fn multiple_identities_have_real_distinct_buffers_and_inventory() {
    let registry = EndpointRegistry::new();
    add(&registry, 1);
    add(&registry, 2);
    let first = registry.callback([1; 16]).unwrap();
    let second = registry.callback([2; 16]).unwrap();
    assert_ne!(first.transport(), second.transport());
    assert_ne!(first.capture_buffer(), second.capture_buffer());
    pcm(&first);
    let mut silence = [1.0; 14];
    assert_eq!(
        unsafe { mr_shared_audio_buffer_read(second.render_buffer(), 0, silence.as_mut_ptr(), 2) },
        0
    );
    assert_eq!(silence.map(f32::to_bits), [0.0_f32; 14].map(f32::to_bits));
    let snapshot = registry.snapshot().unwrap();
    assert_eq!(snapshot.entries().len(), 2);
    assert_eq!(snapshot.entries()[0].endpoint_id, [1; 16]);
    assert_eq!(snapshot.entries()[1].endpoint_id, [2; 16]);
    assert!(
        snapshot
            .entries()
            .iter()
            .all(|entry| entry.sample_rate == 96_000)
    );
}

#[test]
fn invalid_or_cancelled_preparation_preserves_live_mapping() {
    let registry = EndpointRegistry::new();
    add(&registry, 1);
    let active = registry.callback([1; 16]).unwrap();
    let before = registry.snapshot().unwrap().as_abi().generation;
    let mut bad = spec(1);
    bad.render_channels = 65;
    assert!(registry.prepare_inventory(&[bad]).is_err());
    let token = registry
        .prepare_inventory_change(&[spec(1)], Some([1; 16]))
        .unwrap();
    let staged = token.callback_pins().pop().unwrap();
    assert_ne!(active.transport(), staged.transport());
    drop(token);
    assert!(registry.state.lock().inventory_pending.is_none());
    assert_eq!(
        registry.callback([1; 16]).unwrap().transport(),
        active.transport()
    );
    assert_eq!(registry.snapshot().unwrap().as_abi().generation, before);
    pcm(&active);
    assert_eq!(registry.collect(), 0);
    drop(staged);
    assert_eq!(registry.collect(), 1);
}

#[test]
fn rename_and_nominal_rate_metadata_reuse_transport_but_geometry_edit_does_not() {
    let registry = EndpointRegistry::new();
    add(&registry, 1);
    let original = registry.callback([1; 16]).unwrap();
    let mut desired = spec(1);
    desired.display_name = "Renamed independent Driver".into();
    desired.sample_rate = 48_000;
    let prepared = registry.prepare_inventory(&[desired.clone()]).unwrap();
    let snapshot = prepared.snapshot().unwrap();
    assert_eq!(snapshot.entries().len(), 1);
    let renamed = prepared.callback_pins().pop().unwrap();
    assert_eq!(renamed.transport(), original.transport());
    assert_ne!(renamed.generation(), original.generation());
    prepared.commit().unwrap();
    desired.capture_channels = 4;
    let changed = registry.prepare_inventory(&[desired]).unwrap();
    assert_ne!(changed.callback_pins()[0].transport(), original.transport());
    drop(changed);
    assert_eq!(
        registry.callback([1; 16]).unwrap().transport(),
        original.transport()
    );
}

#[test]
fn removed_mapping_is_reclaimed_only_by_control_after_callback_drops() {
    let registry = EndpointRegistry::new();
    add(&registry, 1);
    let callback = registry.callback([1; 16]).unwrap();
    let weak = Arc::downgrade(&callback.mapping);
    registry.prepare_inventory(&[]).unwrap().commit().unwrap();
    assert!(registry.callback([1; 16]).is_none());
    assert_eq!(registry.collect(), 0);
    // A removed device still has valid native storage for its in-flight graph.
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                pcm(&callback);
                drop(callback);
            })
            .join()
            .unwrap();
    });
    assert!(
        weak.upgrade().is_some(),
        "callback drop must not free the mapping"
    );
    assert_eq!(registry.collect(), 1);
    assert!(weak.upgrade().is_none());
}

struct RemoteMap {
    pointer: *mut c_void,
    length: usize,
}
impl Drop for RemoteMap {
    fn drop(&mut self) {
        assert_eq!(unsafe { munmap(self.pointer, self.length) }, 0);
    }
}

#[test]
fn snapshot_descriptors_outlive_registry_and_attach_real_transport_geometry() {
    let snapshot = {
        let registry = EndpointRegistry::new();
        add(&registry, 4);
        registry.snapshot().unwrap()
    };
    let entry = &snapshot.entries()[0];
    let file = File::from(
        unsafe { BorrowedFd::borrow_raw(entry.descriptor) }
            .try_clone_to_owned()
            .unwrap(),
    );
    assert!(file.metadata().unwrap().len() >= entry.mapped_size as u64);
    let pointer = unsafe {
        mmap(
            ptr::null_mut(),
            entry.mapped_size,
            3,
            1,
            file.as_raw_fd(),
            0,
        )
    };
    assert_ne!(pointer, ptr::without_provenance_mut(usize::MAX));
    let remote = RemoteMap {
        pointer,
        length: entry.mapped_size,
    };
    drop(snapshot);
    drop(file);
    let transport = unsafe { mr_endpoint_transport_attach(remote.pointer, remote.length) };
    assert!(!transport.is_null());
    let mut config = EndpointTransportConfig {
        endpoint_id: [0; 16],
        capacity_frames: 0,
        render_channels: 0,
        capture_channels: 0,
    };
    assert!(unsafe { mr_endpoint_transport_config(transport, &raw mut config) });
    assert_eq!(config.endpoint_id, [4; 16]);
    assert_eq!(config.render_channels, 7);
    assert_eq!(config.capture_channels, 2);
    assert_eq!(config.capacity_frames, 64);
}

#[test]
fn exhausted_epoch_replaces_mapping_and_preserves_full_endpoint_spec() {
    let registry = EndpointRegistry::new();
    add(&registry, 1);
    let old = registry.callback([1; 16]).unwrap();
    // Exercise the actual shared buffer boundary, not an artificial registry flag.
    for _ in 1..u16::MAX {
        assert_ne!(
            unsafe { mr_shared_audio_buffer_try_begin_epoch(old.capture_buffer()) },
            0
        );
    }
    assert_eq!(
        unsafe { mr_shared_audio_buffer_try_begin_epoch(old.capture_buffer()) },
        0
    );
    let replacement = registry
        .prepare_inventory_change(&[spec(1)], Some([1; 16]))
        .unwrap();
    replacement.commit().unwrap();
    let new = registry.callback([1; 16]).unwrap();
    assert_eq!(new.spec(), old.spec());
    assert_ne!(new.transport(), old.transport());
    assert!(new.generation() > old.generation());
    // The old remote timeline never wraps back to generation one.
    assert_eq!(
        unsafe { mr_shared_audio_buffer_try_begin_epoch(old.capture_buffer()) },
        0
    );
    assert_ne!(
        unsafe { mr_shared_audio_buffer_try_begin_epoch(new.capture_buffer()) },
        0
    );
    assert_eq!(registry.collect(), 0);
    drop(old);
    assert_eq!(registry.collect(), 1);
}

#[test]
fn generation_exhaustion_rejects_before_any_mapping_is_published() {
    let registry = EndpointRegistry::new();
    add(&registry, 1);
    let old = registry.callback([1; 16]).unwrap();
    registry.state.lock().generation = u64::MAX;
    assert!(
        registry
            .prepare_inventory_change(&[spec(1)], Some([1; 16]))
            .is_err()
    );
    assert!(registry.prepare_inventory(&[]).is_err());
    assert_eq!(
        registry.callback([1; 16]).unwrap().transport(),
        old.transport()
    );
    assert!(registry.state.lock().inventory_pending.is_none());
    assert_eq!(registry.snapshot().unwrap().as_abi().generation, u64::MAX);
}

#[test]
fn invalid_identity_name_rate_and_unidirectional_geometry_are_validated() {
    let registry = EndpointRegistry::new();
    for bad in [
        EndpointSpec {
            id: [0; 16],
            ..spec(1)
        },
        EndpointSpec {
            display_name: "bad\0name".into(),
            ..spec(1)
        },
        EndpointSpec {
            sample_rate: 384_000,
            ..spec(1)
        },
        EndpointSpec {
            capacity_frames: 63,
            ..spec(1)
        },
        EndpointSpec {
            render_channels: 0,
            capture_channels: 0,
            ..spec(1)
        },
    ] {
        assert!(registry.prepare_inventory(&[bad]).is_err());
    }
    let token = registry
        .prepare_inventory(&[EndpointSpec {
            render_channels: 0,
            ..spec(1)
        }])
        .unwrap();
    token.commit().unwrap();
    let callback = registry.callback([1; 16]).unwrap();
    assert!(callback.render_buffer().is_null());
    assert!(!callback.capture_buffer().is_null());
}

#[test]
fn preparations_are_bound_to_owner_and_commit_or_abort_exactly_once() {
    let first = EndpointRegistry::new();
    let second = EndpointRegistry::new();
    let first_prepared = first.prepare_inventory(&[spec(1)]).unwrap();
    let second_prepared = second.prepare_inventory(&[spec(1)]).unwrap();
    let first_pin = first_prepared.callback_pins().pop().unwrap();
    let weak = Arc::downgrade(&first_pin.mapping);
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _transaction = first_prepared;
        panic!("graph preparation failed before commit");
    }));
    assert!(failure.is_err());
    assert!(first.state.lock().inventory_pending.is_none());
    assert!(first.callback([1; 16]).is_none());
    assert_eq!(first.collect(), 0);
    drop(first_pin);
    assert_eq!(first.collect(), 1);
    assert!(weak.upgrade().is_none());

    // Both internal counters started at one, but no public token can target the
    // wrong owner. The other registry's transaction remains independently live.
    assert!(second.state.lock().inventory_pending.is_some());
    second_prepared.commit().unwrap();
    assert!(second.state.lock().inventory_pending.is_none());
    assert_eq!(second.snapshot().unwrap().as_abi().generation, 2);
    pcm(&second.callback([1; 16]).unwrap());
    assert_eq!(
        second.collect(),
        0,
        "committing must not cancel the active map"
    );
}

#[test]
fn exclusive_inventory_reservation_cancels_without_resurrecting_removed_devices() {
    let registry = EndpointRegistry::new();
    add(&registry, 1);
    let pending = registry
        .prepare_inventory_change(&[spec(1)], Some([1; 16]))
        .unwrap();
    let pin = pending.callback_pins().pop().unwrap();
    assert!(registry.prepare_inventory(&[]).is_err());
    drop(pending);
    registry.prepare_inventory(&[]).unwrap().commit().unwrap();
    assert!(registry.callback([1; 16]).is_none());
    assert!(registry.state.lock().inventory_pending.is_none());
    pcm(&pin);
    drop(pin);
    assert_eq!(registry.collect(), 2);
}

#[test]
fn cancellation_of_unpublished_inventory_preserves_generation_and_pins() {
    let registry = EndpointRegistry::new();
    let before = registry.snapshot().unwrap().as_abi().generation;
    let pending = registry.prepare_inventory(&[spec(1)]).unwrap();
    let pin = pending.callback_pins().pop().unwrap();
    drop(pending);
    assert!(registry.state.lock().inventory_pending.is_none());
    let inventory = registry.snapshot().unwrap();
    assert!(inventory.entries().is_empty());
    assert_eq!(inventory.as_abi().generation, before);
    pcm(&pin);
    assert_eq!(registry.collect(), 0);
    drop(pin);
    assert_eq!(registry.collect(), 1);
}

#[test]
fn batch_changes_are_atomic_and_unchanged_endpoint_retains_mapping_and_rate() {
    let registry = EndpointRegistry::new();
    registry
        .prepare_inventory(&[spec(1), spec(2)])
        .unwrap()
        .commit()
        .unwrap();
    let before = registry.snapshot().unwrap();
    let untouched = registry.callback([2; 16]).unwrap();
    let mut changed = spec(1);
    changed.display_name = "Renamed globally".into();
    changed.sample_rate = 44_100;
    let update = registry
        .prepare_inventory(&[changed.clone(), spec(2), spec(3)])
        .unwrap();
    assert_eq!(registry.snapshot().unwrap().entries().len(), 2);
    update.commit().unwrap();
    let after = registry.snapshot().unwrap();
    assert_eq!(after.as_abi().generation, before.as_abi().generation + 1);
    assert_eq!(after.entries().len(), 3);
    assert_eq!(registry.callback([1; 16]).unwrap().spec(), &changed);
    assert_eq!(
        registry.callback([2; 16]).unwrap().transport(),
        untouched.transport()
    );
    registry
        .prepare_inventory(&[spec(2)])
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(registry.snapshot().unwrap().entries().len(), 1);
    assert_eq!(
        registry.callback([2; 16]).unwrap().transport(),
        untouched.transport()
    );
    pcm(&untouched);
    let before_removal = unsafe { mr_shared_audio_buffer_stats(untouched.render_buffer()) };
    assert_eq!(before_removal.published_through_frame, 2);
    registry.prepare_inventory(&[]).unwrap().commit().unwrap();
    assert!(registry.snapshot().unwrap().entries().is_empty());
    let after_removal = unsafe { mr_shared_audio_buffer_stats(untouched.render_buffer()) };
    assert_eq!(after_removal.epoch, before_removal.epoch);
    assert_eq!(after_removal.published_through_frame, 2);
    pcm(&untouched);
}

#[test]
fn duplicate_or_invalid_batch_never_publishes_partial_desired_inventory() {
    let registry = EndpointRegistry::new();
    add(&registry, 1);
    let before = registry.callback([1; 16]).unwrap();
    assert!(registry.prepare_inventory(&[spec(2), spec(2)]).is_err());
    assert!(
        registry
            .prepare_inventory(&[
                spec(2),
                EndpointSpec {
                    capture_channels: 65,
                    ..spec(3)
                }
            ])
            .is_err()
    );
    assert!(
        registry
            .prepare_inventory_change(&[spec(1)], Some([99; 16]))
            .is_err()
    );
    assert!(registry.state.lock().inventory_pending.is_none());
    assert_eq!(registry.snapshot().unwrap().entries().len(), 1);
    assert_eq!(
        registry.callback([1; 16]).unwrap().transport(),
        before.transport()
    );
    pcm(&before);
}
