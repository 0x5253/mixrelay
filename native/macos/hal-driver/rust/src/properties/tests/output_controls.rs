//! Output volume and mute control objects through the property ABI.
use super::{global_address, set_global};
use crate::abi::OK;
use crate::properties::ffi::mr_hal_core_property_get;
use crate::tests::Fixture;
fn conversion(fixture: &Fixture, object: u32, selector: [u8; 4], input: f32) -> f32 {
    let address = global_address(selector);
    let mut value = input;
    let mut written = 0;
    // SAFETY: the in/out Float32 is exclusively owned, aligned and initialized.
    assert_eq!(
        unsafe {
            mr_hal_core_property_get(
                (&raw const *fixture.registry).cast_mut(),
                object,
                &raw const address,
                0,
                std::ptr::null(),
                4,
                &raw mut written,
                (&raw mut value).cast(),
                0,
            )
        },
        OK
    );
    assert_eq!(written, 4);
    value
}

#[test]
fn output_controls_advertise_owned_sdk_classes_scope_and_native_state() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let second = fixture.add(2, 48_000);
    let read_u32 = |object, selector| {
        u32::from_ne_bytes(fixture.property::<4>(object, &global_address(selector)))
    };
    assert_eq!(
        fixture.property::<8>(id, &global_address(*b"ctrl")),
        [id + 3, id + 4]
            .into_iter()
            .flat_map(u32::to_ne_bytes)
            .collect::<Vec<_>>()[..]
    );
    assert_eq!(
        fixture.property::<16>(id, &global_address(*b"ownd")),
        [id + 1, id + 2, id + 3, id + 4]
            .into_iter()
            .flat_map(u32::to_ne_bytes)
            .collect::<Vec<_>>()[..]
    );
    assert_eq!(read_u32(id + 3, *b"clas"), u32::from_be_bytes(*b"vlme"));
    assert_eq!(read_u32(id + 3, *b"bcls"), u32::from_be_bytes(*b"levl"));
    assert_eq!(read_u32(id + 4, *b"clas"), u32::from_be_bytes(*b"mute"));
    assert_eq!(read_u32(id + 4, *b"bcls"), u32::from_be_bytes(*b"togl"));
    assert_eq!(read_u32(id + 3, *b"stdv"), id);
    assert_eq!(read_u32(id + 3, *b"cscp"), u32::from_be_bytes(*b"outp"));
    assert_eq!(read_u32(id + 4, *b"celm"), 0);
    assert_eq!(
        set_global(&fixture, id + 3, *b"lcsv", &0.5_f32.to_ne_bytes()),
        OK
    );
    let db = f32::from_ne_bytes(fixture.property::<4>(id + 3, &global_address(*b"lcdv")));
    assert!((db + 6.0206).abs() < 0.0001);
    assert!((conversion(&fixture, id + 3, *b"lcsd", 0.5) - db).abs() < 0.0001);
    assert!((conversion(&fixture, id + 3, *b"lcds", db) - 0.5).abs() < 0.0001);
    assert_eq!(
        set_global(&fixture, id + 4, *b"bcvl", &1_u32.to_ne_bytes()),
        OK
    );
    assert_eq!(read_u32(id + 4, *b"bcvl"), 1);
    assert_eq!(fixture.registry.output_control(id), Some((0.5, true)));
    assert_eq!(fixture.registry.output_control(second), Some((1.0, false)));
    assert!(crate::device::lock(&fixture.host.changes).contains(&(id, crate::abi::OUTPUT_VOLUME)));
    assert!(crate::device::lock(&fixture.host.changes).contains(&(id, crate::abi::OUTPUT_MUTE)));
}

#[test]
fn output_control_setters_reject_wrong_objects_sizes_and_invalid_levels() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    for value in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
        assert_ne!(
            set_global(&fixture, id + 3, *b"lcsv", &value.to_ne_bytes()),
            OK
        );
    }
    for value in [f32::NAN, -97.0, 1.0] {
        assert_ne!(
            set_global(&fixture, id + 3, *b"lcdv", &value.to_ne_bytes()),
            OK
        );
    }
    assert_ne!(set_global(&fixture, id + 3, *b"lcsv", &[0; 3]), OK);
    assert_ne!(
        set_global(&fixture, id + 4, *b"lcsv", &0.5_f32.to_ne_bytes()),
        OK
    );
    assert_ne!(set_global(&fixture, id, *b"bcvl", &1_u32.to_ne_bytes()), OK);
    assert_eq!(
        set_global(&fixture, id + 3, *b"lcdv", &(-6.0206_f32).to_ne_bytes()),
        OK
    );
    assert!((fixture.registry.output_control(id).unwrap().0 - 0.5).abs() < 0.0001);
    assert_eq!(fixture.registry.remove([1; 16]), OK);
    fixture.registry.tick();
    assert_ne!(
        set_global(&fixture, id + 3, *b"lcsv", &0.5_f32.to_ne_bytes()),
        OK
    );
}

#[test]
fn capture_only_devices_do_not_advertise_native_output_controls() {
    let fixture = Fixture::new();
    let id = fixture
        .registry
        .upsert([1; 16], "Capture only", 48_000, (fixture.fd(1, 0, 2), 24))
        .unwrap();
    assert_eq!(fixture.property::<0>(id, &global_address(*b"ctrl")), []);
    assert_eq!(
        fixture.property::<4>(id, &global_address(*b"ownd")),
        (id + 1).to_ne_bytes()
    );
    let address = global_address(*b"lcsv");
    assert!(!unsafe {
        crate::properties::ffi::mr_hal_core_has_property(
            (&raw const *fixture.registry).cast_mut(),
            id + 3,
            &raw const address,
        )
    });
    assert_eq!(
        fixture.registry.set_output_volume(id + 3, 0.5),
        crate::abi::NOT_FOUND
    );
}
