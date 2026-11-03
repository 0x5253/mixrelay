use super::*;

#[test]
fn partial_system_listener_restore_retries_before_reopening_pcm() {
    for boundary in [
        Failure::ServiceListener,
        Failure::InventoryListener,
        Failure::SystemInstall,
    ] {
        let session = fallback_fixture();
        let native = Fake::default();
        *native.selected.borrow_mut() = session
            .selection
            .borrow()
            .as_ref()
            .unwrap()
            .selected
            .clone();
        let mut resources = session.resources.get();
        resources.listeners.service = true;
        session.resources.set(resources);
        native::service_restarted(&session);
        native.fail.set(boundary);
        assert!(session.refresh(&native).is_err());
        assert!(!session.allowed());
        assert_eq!(native.rebuild_calls.get(), 0);
        native.fail.set(Failure::None);
        session.refresh(&native).unwrap();
        let resources = session.resources.get();
        assert!(resources.listeners.service && resources.listeners.inventory);
        assert_eq!(
            resources.listeners.service_epoch,
            session.service_epoch.load(Ordering::Acquire)
        );
        assert_eq!(native.rebuild_calls.get(), 1);
        assert!(session.allowed());
        assert!(session.reclaim(&native));
    }
}

#[test]
fn service_restart_rebuilds_unchanged_native_bundle_and_process_list_taps() {
    for fallback in [false, true] {
        let session = fallback_fixture();
        session.selection.borrow_mut().as_mut().unwrap().fallback = fallback;
        let native = Fake::default();
        *native.selected.borrow_mut() = session
            .selection
            .borrow()
            .as_ref()
            .unwrap()
            .selected
            .clone();
        let measured = allocation_counter::measure(|| native::service_restarted(&session));
        assert_eq!(measured.count_total, 0);
        assert!(!session.allowed());
        native.fail.set(Failure::Unregister);
        assert!(session.refresh(&native).is_err());
        assert!(!session.allowed());
        assert_eq!(native.rebuild_calls.get(), 0);
        native.fail.set(Failure::None);
        session.refresh(&native).unwrap();
        assert_eq!(native.rebuild_calls.get(), 1);
        assert!(session.allowed());
        assert!(session.running.load(Ordering::Acquire));
        session.refresh(&native).unwrap();
        assert_eq!(
            native.rebuild_calls.get(),
            1,
            "unchanged tap rebuilt repeatedly"
        );
        assert!(session.reclaim(&native));
    }
}

#[test]
fn deleted_objects_release_native_receipts_only_after_local_callbacks_and_listeners_drain() {
    for status in [i32::from_be_bytes(*b"!dev"), i32::from_be_bytes(*b"!obj")] {
        let session = fixture();
        let native = Fake::default();
        native.gone_status.set(status);
        session.lifecycle.retire();
        session.active_callbacks.store(1, Ordering::Release);
        assert!(!session.reclaim(&native));
        assert_ne!(session.resources.get().aggregate, 0);
        assert!(
            session.resources.get().proc.is_null(),
            "deleted registration retained forever"
        );
        session.active_callbacks.store(0, Ordering::Release);
        session.active_listeners.store(1, Ordering::Release);
        assert!(!session.reclaim(&native));
        assert_ne!(session.resources.get().tap, 0);
        session.active_listeners.store(0, Ordering::Release);
        assert!(session.reclaim(&native));
        assert_eq!(session.resources.get().tap, 0);
        assert_eq!(session.resources.get().aggregate, 0);
    }
}

#[test]
fn an_arbitrary_native_error_never_acknowledges_deletion() {
    let session = fixture();
    let native = Fake::default();
    session.lifecycle.retire();
    native.gone_status.set(i32::from_be_bytes(*b"nope"));
    assert!(!session.reclaim(&native));
    assert!(!session.resources.get().proc.is_null());
    assert_ne!(session.resources.get().aggregate, 0);
    native.gone_status.set(0);
    assert!(session.reclaim(&native));
}
