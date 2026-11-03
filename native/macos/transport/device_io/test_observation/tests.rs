use super::*;
use std::{ptr, sync::atomic::Ordering::Acquire};

#[test]
fn explicit_fixture_admission_uses_the_native_rate_and_channel_validator() {
    // SAFETY: null fixtures own no resource; each non-null owner is unpublished and drained.
    assert!(!unsafe { admit_float_callback(ptr::null_mut()) });
    for channels in [1, 2, 32, 64] {
        for (rate, accepted) in [(1, false), (8_000, true), (96_000, true), (384_001, false)] {
            let io = DeviceIo::for_test(c"synthetic-admission", true, channels, rate).unwrap();
            // SAFETY: this local fixture exclusively owns all cells and has no registration.
            assert_eq!(
                unsafe { admit_float_callback(ptr::from_ref(&io).cast_mut()) },
                accepted
            );
        }
    }
}

#[test]
fn removal_fires_listeners_and_return_reregisters_on_the_new_object() {
    let hal = SimulatedHal::new();
    let first = hal.plug("io.mixrelay.test.device", 2, 48_000, 0.25);
    let io = DeviceIo::for_test(c"io.mixrelay.test.device", true, 2, 44_100)
        .expect("device owner")
        .publish();
    let invalid = |io: *mut DeviceIo| {
        // SAFETY: the test is the sole control owner, so it may read the connection's
        // listener latch; the latch itself is atomic.
        unsafe { &*(*io).control.get() }
            .connection
            .watch_invalid
            .load(Acquire)
    };
    // SAFETY: this test is the owner's sole control worker; no callback runs here.
    unsafe {
        assert_eq!(connect_status(&hal, io, 0), 0);
        assert_eq!(super::super::mr_device_io_native_rate(io), 48_000);
        assert!(alive(&hal, io));
        assert_eq!(clock_device(&hal, io), first);
        assert!(hal.watches() > 0, "registration watches its device");
        assert_eq!(
            hal.stream(io),
            Some(Stream {
                procedure: 1,
                rate: 48_000,
                frames: DEFAULT_BUFFER,
                capture: vec![0.25; 2],
            })
        );
        hal.unplug("io.mixrelay.test.device");
        // The removal listener invalidates the registration before any poll.
        assert!(invalid(io));
        // An absent input buffer still lets the next callback publish its
        // registration-local notification through the stable graph-facing latch.
        let allocated = allocation_counter::measure(|| {
            super::super::mr_device_io_process(
                first,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null(),
                io.cast(),
            );
        });
        assert_eq!(allocated.count_total, 0);
        assert!(super::super::mr_device_io_invalid(io));
        assert!(!alive(&hal, io));
        assert_eq!(hal.stream(io), None);
        assert_eq!(connect_status(&hal, io, 0), -1, "registration not drained");
        // Removed objects acknowledge unregistration and listener removal.
        assert!(disconnect(&hal, io));
        assert_eq!(hal.watches(), 0, "a removed device's listener leaked");
        assert_eq!(connect_status(&hal, io, 0), -2, "absent device registered");
        assert_eq!(hal.replug("io.mixrelay.test.device"), Some(DEFAULT_BUFFER));
        assert_eq!(connect_status(&hal, io, 0), 0);
        assert!(!invalid(io), "the new registration owns a fresh latch");
        assert!(!super::super::mr_device_io_invalid(io));
        let second = clock_device(&hal, io);
        assert!(second != 0 && second != first, "stale object reused");
        assert_eq!(
            direct_binding(&hal, io),
            Some(NativeBinding {
                device: second,
                offset: 0,
                total: 0,
            })
        );
        assert_eq!(hal.stream(io).map(|stream| stream.procedure), Some(2));
        assert!(destroy(&hal, io));
    }
    assert_eq!(hal.watches(), 0, "destroyed owner's listener leaked");
}

#[test]
fn unsupported_format_fails_preparation_without_leaving_a_registration() {
    let hal = SimulatedHal::new();
    hal.plug("io.mixrelay.test.compressed", 2, 48_000, 0.25);
    hal.publish_unsupported_format("io.mixrelay.test.compressed");
    let io = DeviceIo::for_test(c"io.mixrelay.test.compressed", false, 2, 48_000)
        .expect("device owner")
        .publish();
    // SAFETY: this test is the owner's sole control worker and its allocation owner.
    unsafe {
        assert_eq!(connect_status(&hal, io, 0), -3);
        assert_eq!(hal.stream(io), None);
        assert_eq!(hal.watches(), 0, "failed preparation kept its listeners");
        assert!(destroy(&hal, io));
    }
}

#[test]
fn buffer_facts_and_writes_follow_the_resolved_device() {
    let hal = SimulatedHal::new();
    let device = hal.plug("io.mixrelay.test.buffer", 2, 48_000, 0.25);
    let io =
        DeviceIo::for_test(c"io.mixrelay.test.buffer", false, 2, 48_000).expect("device owner");
    let pointer = ptr::from_ref(&io).cast_mut();
    let mut info = BufferInfo::default();
    // SAFETY: the local owner is retained and `info` is exclusive local storage.
    assert!(unsafe { buffer_info(&hal, pointer, &raw mut info) });
    assert_eq!(
        (info.device, info.frames, info.settable),
        (device, DEFAULT_BUFFER, 1)
    );
    assert_eq!((info.minimum, info.maximum), (15.0, 4096.0));
    assert!(buffer_write(&hal, device, 256));
    assert!(
        !buffer_write(&hal, device, 8192),
        "out-of-range buffer accepted"
    );
    assert_eq!(buffer_size(&hal, device), 256);
    assert_eq!(hal.buffer("io.mixrelay.test.buffer"), Some(256));
    hal.unplug("io.mixrelay.test.buffer");
    // SAFETY: as above.
    assert!(!unsafe { buffer_info(&hal, pointer, &raw mut info) });
    assert_eq!((info.device, info.frames), (0, 0));
}

#[test]
fn null_owner_buffer_query_zeroes_a_previously_populated_output() {
    let hal = SimulatedHal::new();
    let mut info = BufferInfo {
        device: 7,
        frames: 256,
        minimum: 15.0,
        maximum: 4096.0,
        settable: 1,
    };
    // SAFETY: a null owner and exclusive local output storage.
    assert!(!unsafe { buffer_info(&hal, ptr::null_mut(), &raw mut info) });
    assert_eq!((info.device, info.frames, info.settable), (0, 0, 0));
    assert_eq!(
        (info.minimum.to_bits(), info.maximum.to_bits()),
        (0_f64.to_bits(), 0_f64.to_bits())
    );
}
