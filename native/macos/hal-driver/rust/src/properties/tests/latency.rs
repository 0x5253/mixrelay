//! Directional latency and its notifications.
use crate::abi::OK;
use crate::properties::Address;
use crate::tests::Fixture;

#[test]
fn latency_properties_are_directional_and_reentrant_without_control_locks() {
    use mixrelay_macos_transport::control_inventory::InputLatency;
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    fixture.registry.publish_latencies(&[InputLatency {
        endpoint_id: [1; 16],
        rate: 48_000,
        engine_pid: 10,
        lease: 20,
        revision: 30,
        frames: Some(321),
    }]);
    let device = fixture.registry.pin(id).unwrap();
    let _control = crate::device::lock(&device.control);
    let _identity = crate::device::lock(&fixture.registry.client_identity_gate);
    let _audio = crate::transport::AudioScope::enter();
    // Application latency metadata does not change the host's capture coordinates.
    let input = Address {
        selector: u32::from_be_bytes(*b"ltnc"),
        scope: u32::from_be_bytes(*b"inpt"),
        element: 0,
    };
    assert_eq!(
        u32::from_ne_bytes(fixture.property_for::<4>(id, &input, 4242)),
        321
    );
    assert_eq!(u32::from_ne_bytes(fixture.property::<4>(id, &input)), 0);
    for (object, selector, scope, expected) in [
        (id, *b"ltnc", *b"outp", 0),
        (id, *b"saft", *b"inpt", 0),
        (id, *b"saft", *b"outp", 0),
        (id + 1, *b"ltnc", *b"glob", 0),
        (id + 2, *b"ltnc", *b"glob", 0),
    ] {
        let address = Address {
            selector: u32::from_be_bytes(selector),
            scope: u32::from_be_bytes(scope),
            element: 0,
        };
        assert_eq!(
            u32::from_ne_bytes(fixture.property::<4>(object, &address)),
            expected
        );
    }
}

/// The host's zero latency stays stable while app measurements and notifications change.
#[test]
fn latency_notifications_preserve_host_coordinates_and_app_measurements() {
    use mixrelay_macos_transport::control_inventory::InputLatency;
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    crate::device::lock(&fixture.host.changes).clear();
    let input = Address {
        selector: u32::from_be_bytes(*b"ltnc"),
        scope: u32::from_be_bytes(*b"inpt"),
        element: 0,
    };
    let app = |fixture: &Fixture| u32::from_ne_bytes(fixture.property_for::<4>(id, &input, 4242));
    let coreaudiod =
        |fixture: &Fixture, pid| u32::from_ne_bytes(fixture.property_for::<4>(id, &input, pid));
    let publish = |frames| {
        fixture.registry.publish_latencies(&[InputLatency {
            endpoint_id: [1; 16],
            rate: 48_000,
            engine_pid: 10,
            lease: 20,
            revision: 30 + u64::from(frames),
            frames: Some(frames),
        }]);
    };
    publish(300);
    assert_eq!((app(&fixture), coreaudiod(&fixture, 0)), (300, 0));
    publish(300);
    assert_eq!((app(&fixture), coreaudiod(&fixture, 0)), (300, 0));
    publish(400);
    assert_eq!((app(&fixture), coreaudiod(&fixture, 0)), (400, 0));
    assert_eq!(
        *crate::device::lock(&fixture.host.changes),
        [
            (id, crate::abi::INPUT_LATENCY),
            (id, crate::abi::INPUT_LATENCY)
        ]
    );
    // coreaudiod may also ask under its own PID, registered with its own bundle.
    let registry = (&raw const *fixture.registry).cast_mut();
    // SAFETY: the fixture keeps the registry alive; the bundle outlives the call.
    let status = unsafe {
        crate::abi::mr_hal_core_client_identity(
            registry,
            id,
            1,
            777,
            c"com.apple.audio.coreaudiod".as_ptr(),
            true,
        )
    };
    assert_eq!(status, OK);
    assert_eq!(coreaudiod(&fixture, 777), 0);
    // Contention on the identity registry never exposes the frames to coreaudiod.
    let held = crate::device::lock(&fixture.registry.transport.state);
    assert_eq!(coreaudiod(&fixture, 777), 0);
    drop(held);
    // Once coreaudiod's registration ends, that PID is an ordinary requester.
    // SAFETY: as above; removal names no bundle.
    let removed = unsafe {
        crate::abi::mr_hal_core_client_identity(registry, id, 1, 777, std::ptr::null(), false)
    };
    assert_eq!(removed, OK);
    assert_eq!(coreaudiod(&fixture, 777), 400);
}
