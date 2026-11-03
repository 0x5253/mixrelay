use super::*;
use crate::device_io::{control, mr_device_io_process};

#[test]
fn failed_stream_configuration_drains_the_partial_registration_before_retry() {
    for input in [false, true] {
        let hal = SimulatedHal::new();
        hal.plug("output", 2, 48_000, 0.25);
        let io = DeviceIo::for_test(c"output", input, 2, 48_000).unwrap();
        io.set_output_only(true);
        hal.state().usage_status = -17;
        // SAFETY: sole control owner retains the fixture and every native registration.
        unsafe {
            assert_eq!(control::connect(&io, 0, &hal), -17);
            assert_eq!(hal.registrations(), 0);
            assert_eq!(hal.watches(), 0);
            hal.state().usage_status = 0;
            assert_eq!(control::connect(&io, 0, &hal), 0);
            assert!(control::alive(&io, &hal));
            assert!(control::disconnect(&io, &hal));
        }
        assert_eq!(hal.registrations(), 0);
        assert_eq!(hal.watches(), 0);
    }
}

#[test]
fn output_only_registration_does_not_disable_another_clients_capture() {
    let hal = SimulatedHal::new();
    hal.plug("duplex", 2, 48_000, 0.25);
    let input = DeviceIo::for_test(c"duplex", true, 2, 48_000).unwrap();
    let output = DeviceIo::for_test(c"duplex", false, 2, 48_000).unwrap();
    output.set_output_only(true);
    // SAFETY: synchronous control fixture retains both owners and drains each registration.
    unsafe {
        assert_eq!(control::connect(&input, 0, &hal), 0);
        assert_eq!(control::connect(&output, 0, &hal), 0);
        {
            let state = hal.state();
            for registration in state.procedures.values() {
                assert_eq!(
                    registration.enabled,
                    if registration.context == ptr::from_ref(&output).addr() {
                        [false, true]
                    } else {
                        [true, false]
                    }
                );
            }
        }
        assert!(control::disconnect(&output, &hal));
        assert!(control::alive(&input, &hal));
        assert!(control::disconnect(&input, &hal));
    }
    assert_eq!(hal.registrations(), 0);
    assert_eq!(hal.watches(), 0);
}

#[test]
fn hal_restart_invalidates_live_same_format_objects_and_fresh_registration_recovers() {
    let hal = SimulatedHal::new();
    let device = hal.plug("restarted", 2, 48_000, 0.25);
    let io = DeviceIo::for_test(c"restarted", false, 2, 48_000).unwrap();
    // SAFETY: sole control owner; fixture invokes and drains the actual native callback.
    unsafe {
        assert_eq!(control::connect(&io, 0, &hal), 0);
        let restart = hal
            .state()
            .watches
            .iter()
            .find(|watch| watch.subscription.target() == (1, u32::from_be_bytes(*b"srst")))
            .expect("service restart watch")
            .block
            .clone();
        let measured = allocation_counter::measure(|| {
            restart.call((1, ptr::null()));
            mr_device_io_process(
                device,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null(),
                ptr::from_ref(&io).cast_mut().cast(),
            );
        });
        assert_eq!(measured.count_total, 0);
        assert!(
            !control::alive(&io, &hal),
            "matching old geometry survived a HAL restart"
        );
        assert!(control::disconnect(&io, &hal));
        assert_eq!(control::connect(&io, 0, &hal), 0);
        restart.call((1, ptr::null()));
        assert!(
            control::alive(&io, &hal),
            "late old notification invalidated new registration"
        );
        assert!(control::disconnect(&io, &hal));
    }
    assert_eq!(hal.registrations(), 0);
    assert_eq!(hal.watches(), 0);
}

#[test]
fn reused_object_with_same_format_cannot_inherit_a_persistent_uid_selection() {
    let hal = SimulatedHal::new();
    let old = hal.plug("selected", 2, 48_000, 0.25);
    let io = DeviceIo::for_test(c"selected", true, 2, 48_000).unwrap();
    // SAFETY: no callbacks run; this fixture owns every control operation and registration.
    unsafe {
        assert_eq!(control::connect(&io, 0, &hal), 0);
        // Simulate a missed removal notification and object ID reuse by a different UID.
        let recycled = hal.state().devices.remove("selected").unwrap();
        hal.state()
            .devices
            .insert("different-device".into(), recycled);
        hal.plug("selected", 2, 48_000, 0.5);
        assert!(hal.state().alive(old));
        assert!(
            !control::alive(&io, &hal),
            "another device inherited the selected UID"
        );
        assert!(control::disconnect(&io, &hal));
        assert_eq!(control::connect(&io, 0, &hal), 0);
        assert!(control::alive(&io, &hal));
        assert!(control::disconnect(&io, &hal));
    }
}
