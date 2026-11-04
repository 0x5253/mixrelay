use super::*;

#[test]
fn failed_teardown_and_partial_create_keep_complete_owner_until_acknowledged() {
    for (create_status, start_status) in [(0, 0), (17, 0), (0, 19)] {
        let pointer = owner(false, 2).publish();
        assert!(!pointer.is_null());
        // SAFETY: test retains published allocation until its successful retirement below.
        let io = unsafe { &*pointer };
        let mut native = RegistrationStub {
            create_status,
            start_status,
            destroy_status: 23,
            ..RegistrationStub::default()
        };
        let uid = io.selection.uid.as_ptr();
        // SAFETY: no callback registered; scratch allocation identity read only here.
        let scratch = unsafe { (*io.callback.get()).scratch.as_ptr() };
        let pcm = [0.0; 128];
        // SAFETY: sole graph/worker producers and complete immutable block.
        unsafe {
            assert_eq!(queues::graph_write(io, pcm.as_ptr(), 64, 0), 64);
            assert_eq!(queues::bridge_write(io, pcm.as_ptr(), 64), 64);
        }
        assert_eq!(register(io, &mut native), create_status.max(start_status));
        assert!(!retire(pointer, &mut native));
        assert_eq!(io.selection.uid.as_ptr(), uid);
        // SAFETY: failed retirement leaves complete allocation and scratch untouched.
        assert_eq!(unsafe { (*io.callback.get()).scratch.as_ptr() }, scratch);
        assert_eq!(io.native_ring.stats().queued, 64);
        assert_eq!(io.graph_ring.stats().queued, 64);
        let prepares = native.prepares;
        assert_eq!(register(io, &mut native), -1);
        assert_eq!(native.prepares, prepares);
        let mut hardware = [1.0; 128];
        process(io, &mut buffers(&mut hardware, 2, 64, false));
        assert!(hardware.iter().all(|sample| sample.to_bits() == 0));
        assert_eq!(io.progress.ticks.load(Acquire), 0);
        native.destroy_status = if create_status != 0 {
            i32::from_be_bytes(*b"!obj")
        } else {
            0
        };
        assert!(retire(pointer, &mut native));
        assert_eq!(native.stops, native.destroys);
    }
}

#[test]
fn absent_device_fails_before_registration_and_releases_owner() {
    let hal = test_observation::SimulatedHal::new();
    // SAFETY: valid immutable UID, then sole control ownership. The empty simulated HAL
    // resolves no device, so preparation fails before any IOProc creation or start.
    unsafe {
        let io = mr_device_io_create(c"io.mixrelay.test.absent".as_ptr(), true, 2, 48000);
        assert!(!io.is_null());
        assert_eq!(test_observation::connect_status(&hal, io, 0), -2);
        assert_eq!(hal.registrations(), 0);
        assert_eq!(mr_device_io_ticks(io), 0);
        assert!(test_observation::destroy(&hal, io));
    }
}
