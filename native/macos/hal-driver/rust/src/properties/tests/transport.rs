//! Per-requester transport type and its lock-free read paths.
use super::global_address;
use crate::abi::OK;
use crate::properties::{Address, ffi::mr_hal_core_property_get};
use crate::tests::Fixture;

#[test]
fn transport_presentation_is_per_requester_and_preserves_property_contract() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let pid = i32::try_from(std::process::id()).unwrap();
    let registry = (&raw const *fixture.registry).cast_mut();
    fixture
        .registry
        .transport_policy(vec!["com.test.App".into()]);
    assert!(crate::device::lock(&fixture.host.changes).contains(&(id, crate::abi::TRANSPORT)));
    assert_eq!(
        unsafe {
            crate::abi::mr_hal_core_client_identity(
                registry,
                id,
                1,
                pid,
                c"com.test.App".as_ptr(),
                true,
            )
        },
        OK
    );
    let read = |pid| transport_read(registry, id, pid);
    assert_eq!(read(pid), u32::from_be_bytes(*b"usb "));
    assert_eq!(read(0), u32::from_be_bytes(*b"virt"));
    assert_eq!(read(i32::MAX), u32::from_be_bytes(*b"virt"));
    assert_eq!(read(pid), u32::from_be_bytes(*b"usb "));
    assert_eq!(
        unsafe {
            crate::abi::mr_hal_core_client_identity(registry, id, 1, 0, std::ptr::null(), false)
        },
        OK
    );
    fixture.registry.transport_policy(vec![]);
    assert_eq!(read(pid), u32::from_be_bytes(*b"virt"));
    assert_eq!(
        unsafe {
            crate::abi::mr_hal_core_client_identity(
                std::ptr::null_mut(),
                id,
                1,
                pid,
                std::ptr::null(),
                true,
            )
        },
        crate::abi::INVALID
    );
    assert_eq!(
        unsafe { crate::abi::mr_hal_core_client_identity(registry, 0, 1, pid, c"".as_ptr(), true) },
        crate::abi::NOT_FOUND
    );
}

fn transport_read(registry: *mut crate::registry::Registry, id: u32, pid: i32) -> u32 {
    let address = global_address(*b"tran");
    let mut value = 0_u32;
    let mut written = 0;
    assert_eq!(
        unsafe {
            mr_hal_core_property_get(
                registry,
                id,
                &raw const address,
                0,
                std::ptr::null(),
                4,
                &raw mut written,
                (&raw mut value).cast(),
                pid,
            )
        },
        OK
    );
    assert_eq!(written, 4);
    value
}

#[test]
fn transport_property_contract_is_unchanged() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let registry = (&raw const *fixture.registry).cast_mut();
    let address = global_address(*b"tran");
    let mut size = 0;
    assert_eq!(
        unsafe {
            crate::properties::ffi::mr_hal_core_property_size(
                registry,
                id,
                &raw const address,
                &raw mut size,
            )
        },
        OK
    );
    assert_eq!(size, 4);
    let mut settable = true;
    assert_eq!(
        unsafe {
            crate::properties::ffi::mr_hal_core_property_settable(
                registry,
                id,
                &raw const address,
                &raw mut settable,
            )
        },
        OK
    );
    assert!(!settable);
    assert!(unsafe {
        crate::properties::ffi::mr_hal_core_has_property(registry, id, &raw const address)
    });
}

#[test]
fn one_device_reports_usb_and_virtual_to_two_live_processes() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let registry = (&raw const *fixture.registry).cast_mut();
    let pid = i32::try_from(std::process::id()).unwrap();
    let mut child = std::process::Command::new("/bin/cat")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let other = i32::try_from(child.id()).unwrap();
    fixture
        .registry
        .transport_policy(vec!["com.test.App".into()]);
    for (client, pid, bundle) in [(1, pid, c"com.test.App"), (2, other, c"com.test.Other")] {
        assert_eq!(
            unsafe {
                crate::abi::mr_hal_core_client_identity(
                    registry,
                    id,
                    client,
                    pid,
                    bundle.as_ptr(),
                    true,
                )
            },
            OK
        );
    }
    assert_eq!(
        transport_read(registry, id, pid),
        u32::from_be_bytes(*b"usb ")
    );
    assert_eq!(
        transport_read(registry, id, other),
        u32::from_be_bytes(*b"virt")
    );
    assert_eq!(
        transport_read(registry, id, pid),
        u32::from_be_bytes(*b"usb ")
    );
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn io_transport_property_contract_avoids_control_and_resolver_locks() {
    use std::sync::mpsc;
    use std::time::Duration;

    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let registry = &*fixture.registry;
    let pid = i32::try_from(std::process::id()).unwrap();
    registry.transport_policy(vec!["com.test.App".into()]);
    registry
        .transport
        .client(id, 1, pid, Some("com.test.App".into()));
    assert_eq!(
        transport_read((&raw const *registry).cast_mut(), id, pid),
        u32::from_be_bytes(*b"usb ")
    );
    let device = registry.pin(id).unwrap();
    let io_pin = device.io_pin().unwrap();
    let control = crate::device::lock(&device.control);
    let resolver = crate::device::lock(&registry.transport.state);
    let identity_gate = crate::device::lock(&registry.client_identity_gate);
    let inventory = crate::device::lock(&registry.inventory);
    let (done, finished) = mpsc::channel();
    let timely = std::thread::scope(|scope| {
        scope.spawn(|| {
            let _audio = crate::transport::AudioScope::enter();
            let registry = (&raw const *registry).cast_mut();
            let address = global_address(*b"tran");
            let mut size = 0;
            let mut settable = true;
            let value = transport_read(registry, id, pid);
            let has = unsafe {
                crate::properties::ffi::mr_hal_core_has_property(registry, id, &raw const address)
            };
            let sized = unsafe {
                crate::properties::ffi::mr_hal_core_property_size(
                    registry,
                    id,
                    &raw const address,
                    &raw mut size,
                )
            };
            let writable = unsafe {
                crate::properties::ffi::mr_hal_core_property_settable(
                    registry,
                    id,
                    &raw const address,
                    &raw mut settable,
                )
            };
            done.send((value, has, sized, size, writable, settable))
                .unwrap();
        });
        let result = finished.recv_timeout(Duration::from_millis(200));
        // Release locks before joining even on failure, so this regression cannot hang.
        drop(inventory);
        drop(identity_gate);
        drop(resolver);
        drop(control);
        drop(io_pin);
        result
    });
    assert_eq!(
        timely.unwrap(),
        (u32::from_be_bytes(*b"virt"), true, OK, 4, OK, false)
    );
    assert_eq!(
        transport_read((&raw const *registry).cast_mut(), id, pid),
        u32::from_be_bytes(*b"usb ")
    );
}

#[test]
fn transport_validation_preserves_errors_and_scopes_without_snapshot() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let registry = (&raw const *fixture.registry).cast_mut();
    let device = fixture.registry.pin(id).unwrap();
    let control = crate::device::lock(&device.control);
    let _audio = crate::transport::AudioScope::enter();
    let check = |object, scope, element, capacity| {
        let address = Address {
            selector: u32::from_be_bytes(*b"tran"),
            scope,
            element,
        };
        let mut written = 99;
        let mut output = 0;
        let status = unsafe {
            mr_hal_core_property_get(
                registry,
                object,
                &raw const address,
                0,
                std::ptr::null(),
                capacity,
                &raw mut written,
                (&raw mut output).cast(),
                0,
            )
        };
        if status == OK {
            assert_eq!(written, 4);
            assert_eq!(output, u32::from_be_bytes(*b"virt"));
        } else {
            assert_eq!(written, 99);
        }
        status
    };
    for scope in [*b"glob", *b"inpt", *b"outp"] {
        assert_eq!(check(id, u32::from_be_bytes(scope), 0, 4), OK);
    }
    let global = u32::from_be_bytes(*b"glob");
    let unknown = i32::from_be_bytes(*b"who?");
    let bad_object = i32::from_be_bytes(*b"!obj");
    for object in [1, id + 1, id + 2, id + 3, id + 4] {
        assert_eq!(check(object, global, 0, 4), unknown);
    }
    assert_eq!(check(id + 5, global, 0, 4), bad_object);
    assert_eq!(check(id + 1_024, global, 0, 4), bad_object);
    assert_eq!(check(id, global, 1, 4), unknown);
    assert_eq!(check(id, u32::from_be_bytes(*b"oops"), 0, 4), unknown);
    assert_eq!(check(id, global, 0, 3), i32::from_be_bytes(*b"!siz"));
    // Zero-channel child objects retain their previous !obj responses.
    device
        .capture_channels
        .store(0, std::sync::atomic::Ordering::Release);
    device
        .render_channels
        .store(0, std::sync::atomic::Ordering::Release);
    for object in id + 1..=id + 4 {
        assert_eq!(check(object, global, 0, 4), bad_object);
    }
    // Retired device metadata remains queryable until its slot is reclaimed.
    device
        .alive
        .store(false, std::sync::atomic::Ordering::Release);
    assert_eq!(check(id, global, 0, 4), OK);
    device
        .alive
        .store(true, std::sync::atomic::Ordering::Release);
    drop(control);
}

#[test]
fn non_io_transport_read_also_avoids_snapshot_and_budgetless_locks() {
    use std::sync::mpsc;
    use std::time::Duration;

    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let pid = i32::try_from(std::process::id()).unwrap();
    let registry = &*fixture.registry;
    registry.transport_policy(vec!["com.test.App".into()]);
    registry
        .transport
        .client(id, 1, pid, Some("com.test.App".into()));
    let device = registry.pin(id).unwrap();
    let control = crate::device::lock(&device.control);
    let resolver = crate::device::lock(&registry.transport.state);
    let (done, finished) = mpsc::channel();
    let timely = std::thread::scope(|scope| {
        scope.spawn(|| {
            done.send(transport_read((&raw const *registry).cast_mut(), id, pid))
                .unwrap();
        });
        let result = finished.recv_timeout(Duration::from_millis(200));
        drop(resolver);
        drop(control);
        result
    });
    assert_eq!(timely.unwrap(), u32::from_be_bytes(*b"virt"));
    assert_eq!(
        transport_read((&raw const *registry).cast_mut(), id, pid),
        u32::from_be_bytes(*b"usb ")
    );
}

#[test]
fn io_transport_read_completes_while_stop_waits_for_callback_pin() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let registry = &*fixture.registry;
    assert_eq!(registry.client(id, 1, true), OK);
    assert_eq!(registry.start(id, 1, 0), OK);
    let device = registry.pin(id).unwrap();
    // Model the callback's pin separately so a regression can be unwound:
    // release it on timeout, allowing StopIO to unlock control and both threads
    // to join. IO/timestamp both hold this same pin through native callbacks.
    let io_pin = device.io_pin().unwrap();
    let (done, finished) = mpsc::channel();
    let (held_control, timely, stopped) = std::thread::scope(|scope| {
        let stop = scope.spawn(|| registry.stop(id, 1));
        let deadline = Instant::now() + Duration::from_secs(1);
        let held_control = loop {
            if device.control.try_lock().is_err() {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::yield_now();
        };
        scope.spawn(|| {
            let _audio = crate::transport::AudioScope::enter();
            let pid = i32::try_from(std::process::id()).unwrap();
            done.send(transport_read((&raw const *registry).cast_mut(), id, pid))
                .unwrap();
        });
        let timely = finished.recv_timeout(Duration::from_millis(200));
        drop(io_pin);
        (held_control, timely, stop.join().unwrap())
    });
    assert!(held_control);
    assert_eq!(timely.unwrap(), u32::from_be_bytes(*b"virt"));
    assert_eq!(stopped, OK);
    assert_eq!(registry.client(id, 1, false), OK);
}
