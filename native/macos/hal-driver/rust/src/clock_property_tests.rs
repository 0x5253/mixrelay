use super::Fixture;
use crate::abi::OK;
use crate::properties::{Address, mr_hal_core_property_get, mr_hal_core_property_settable};

#[test]
fn host_clock_metadata_is_unfiltered_read_only_and_available_during_control_freeze() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let pin = fixture.registry.pin(id).unwrap();
    let control = super::lock(&pin.control);
    let audio = crate::transport::AudioScope::enter();
    for (selector, expected) in [
        (*b"clok", u32::from_be_bytes(*b"raww")),
        (*b"ring", 16_384),
        (*b"cstb", 1),
    ] {
        let address = Address {
            selector: u32::from_be_bytes(selector),
            scope: u32::from_be_bytes(*b"glob"),
            element: 0,
        };
        let mut output = 0_u32;
        let (mut written, mut writable) = (0, true);
        let registry = (&raw const *fixture.registry).cast_mut();
        let measured = allocation_counter::measure(|| {
            // SAFETY: the fixture and fixed-size outputs live through each synchronous call.
            unsafe {
                assert_eq!(
                    mr_hal_core_property_get(
                        registry,
                        id,
                        &raw const address,
                        0,
                        std::ptr::null(),
                        4,
                        &raw mut written,
                        (&raw mut output).cast(),
                        0
                    ),
                    OK
                );
                assert_eq!(
                    mr_hal_core_property_settable(
                        registry,
                        id,
                        &raw const address,
                        &raw mut writable
                    ),
                    OK
                );
            }
        });
        assert_eq!(measured.count_total, 0);
        assert_eq!((output, written, writable), (expected, 4, false));
    }
    drop(audio);
    drop(control);
}
