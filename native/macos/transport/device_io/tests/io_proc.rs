use super::*;

#[test]
fn callback_uses_direction_timestamp_and_keeps_discontinuities() {
    for (input, flags) in [
        (false, Some(2)),
        (true, Some(2)),
        (true, None),
        (false, Some(0)),
    ] {
        let io = owner(input, 2);
        let now = flags.map(|flags| callback::Timestamp {
            host: 9000,
            flags,
            ..callback::Timestamp::default()
        });
        let mut samples = [0.0; 128];
        let mut list = buffers(&mut samples, 2, 64, false);
        let mut capture = callback::Timestamp {
            sample: 100.0,
            host: 1000,
            flags: 3,
            ..callback::Timestamp::default()
        };
        let mut render = callback::Timestamp {
            sample: 200.0,
            host: 2000,
            flags: 3,
            ..callback::Timestamp::default()
        };
        for pass in 0..2 {
            // SAFETY: complete synchronous fixture lists/timestamps and one callback owner.
            unsafe {
                mr_device_io_process(
                    123,
                    now.as_ref()
                        .map_or(ptr::null(), |time| ptr::from_ref(time).cast()),
                    ptr::from_ref(&list).cast(),
                    ptr::from_ref(&capture).cast(),
                    ptr::from_mut(&mut list).cast(),
                    ptr::from_ref(&render).cast(),
                    ptr::from_ref(&io).cast_mut().cast(),
                );
            }
            let mut clock = CallbackClockSnapshot::default();
            // SAFETY: atomic clock snapshot and exclusive disjoint local output.
            assert!(unsafe { mr_callback_clock_snapshot(&raw const io.clock, &raw mut clock) });
            assert_eq!(clock.valid, 1);
            assert_eq!(
                clock.sample_time.to_bits(),
                if input { capture.sample } else { render.sample }.to_bits()
            );
            assert_eq!(
                clock.host_time,
                if input { capture.host } else { render.host }
            );
            assert_eq!(clock.discontinuities, pass);
            let mut record = ClockBreakRecord::default();
            // SAFETY: the fixture retains IO and exclusively owns disjoint evidence output.
            let recorded =
                unsafe { mr_device_io_next_clock_break(ptr::from_ref(&io), 0, &raw mut record) };
            assert_eq!(recorded, pass != 0);
            if recorded {
                let event = record.event;
                assert_eq!((record.next_cursor, record.missed_before), (1, 0));
                assert_eq!(event.count, pass);
                assert!((event.actual_sample - event.expected_sample - 64.0).abs() < f64::EPSILON);
                assert_eq!(event.actual_host - event.previous_host, 1000);
                assert_eq!(event.flags, 3);
                assert_eq!(event.callback_host, if now.is_some() { 9000 } else { 0 });
                assert_eq!(event.callback_flags, u64::from(flags.unwrap_or(0)));
            }
            capture.sample += 128.0;
            render.sample += 128.0;
            capture.host += 1000;
            render.host += 1000;
        }
    }
}

#[test]
fn callback_queue_pair_is_allocation_free_and_invalid_construction_is_rejected() {
    let io = owner(false, 2);
    let mut hardware = [0.0; 128];
    let source = [0.25; 128];
    let mut list = buffers(&mut hardware, 2, 64, false);
    let allocation = allocation_counter::measure(|| {
        for _ in 0..1000 {
            // SAFETY: serialized producer and exact-sized source.
            unsafe {
                assert_eq!(queues::bridge_write(&io, source.as_ptr(), 64), 64);
            }
            process(&io, &mut list);
        }
    });
    assert_eq!(allocation.count_total, 0);
    assert_pcm(&hardware, &source);
    for channels in [0, 65] {
        assert!(DeviceIo::for_test(c"unit", true, channels, 48000).is_none());
    }
    assert!(DeviceIo::for_test(c"unit", true, 2, 0).is_none());
}

#[test]
fn direct_hook_advances_clock_without_consuming_queued_output() {
    let io = owner(false, 2);
    let calls = Cell::new(0_u32);
    let mut hardware = [0.0; 128];
    let source = [0.25; 128];
    let mut list = buffers(&mut hardware, 2, 64, false);
    // SAFETY: no native registration exists, hook context outlives all test callbacks.
    unsafe {
        mr_device_io_render_hook(
            ptr::from_ref(&io).cast_mut(),
            ptr::from_ref(&calls).cast_mut().cast(),
            Some(render_hook),
        );
        assert_eq!(queues::bridge_write(&io, source.as_ptr(), 64), 64);
    }
    process(&io, &mut list);
    assert_eq!(calls.get(), 1);
    assert!(
        hardware
            .iter()
            .all(|sample| sample.to_bits() == 0.5_f32.to_bits())
    );
    assert_eq!(io.native_ring.stats().queued, 64);
    assert_eq!(io.progress.native_frames.load(Acquire), 64);
    assert_eq!(io.progress.ticks.load(Acquire), 1);
}

#[test]
fn null_owner_buffer_info_initializes_an_uninitialized_output_slot() {
    let mut result = std::mem::MaybeUninit::<native::BufferInfo>::uninit();
    // SAFETY: a null owner makes no device query; the slot is aligned, exclusive and
    // output-only.
    let found = unsafe { mr_device_io_buffer_info(ptr::null_mut(), result.as_mut_ptr()) };
    assert!(!found);
    // SAFETY: a non-null output is always fully initialized, even for a null owner.
    let result = unsafe { result.assume_init() };
    assert_eq!((result.device, result.frames, result.settable), (0, 0, 0));
    assert_eq!(result.minimum.to_bits(), 0_f64.to_bits());
    assert_eq!(result.maximum.to_bits(), 0_f64.to_bits());
}

#[test]
fn null_owner_buffer_info_zeroes_a_previously_populated_output() {
    let mut info = native::BufferInfo {
        device: 7,
        frames: 256,
        minimum: 15.0,
        maximum: 4096.0,
        settable: 1,
    };
    // SAFETY: a null owner makes no device query; `info` is exclusive local storage.
    assert!(!unsafe { mr_device_io_buffer_info(ptr::null_mut(), &raw mut info) });
    assert_eq!((info.device, info.frames, info.settable), (0, 0, 0));
    assert_eq!(
        (info.minimum.to_bits(), info.maximum.to_bits()),
        (0_f64.to_bits(), 0_f64.to_bits())
    );
}

#[test]
fn device_credit_notifies_only_its_owned_signal_without_allocating() {
    let first_signals = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let second_signals = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut first = owner(true, 2);
    let mut second = owner(true, 2);
    let counted = Arc::clone(&first_signals);
    first.set_credit_signal(Arc::new(move || {
        counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));
    let counted = Arc::clone(&second_signals);
    second.set_credit_signal(Arc::new(move || {
        counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));
    let mut source = samples(2, 64);
    let mut list = buffers(&mut source, 2, 64, false);
    let measured = allocation_counter::measure(|| process(&first, &mut list));
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
    assert_eq!(first_signals.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(second_signals.load(std::sync::atomic::Ordering::Relaxed), 0);
    process(&second, &mut list);
    assert_eq!(first_signals.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(second_signals.load(std::sync::atomic::Ordering::Relaxed), 1);
}

fn native_time(sample: f64, host: u64, rate_scalar: f64) -> callback::Timestamp {
    callback::Timestamp {
        sample,
        host,
        rate_scalar,
        flags: 7,
        ..Default::default()
    }
}

#[test]
fn native_timing_reaches_render_hook_without_heap_operations() {
    unsafe extern "C" fn timing_hook(
        context: *mut c_void,
        _: *const c_void,
        _: *mut c_void,
        _: u32,
        timing: *const IoTiming,
    ) -> bool {
        // SAFETY: the fixture retains this Cell and callback-local IoTiming for the call.
        unsafe { &*context.cast::<Cell<IoTiming>>() }.set(unsafe { *timing });
        true
    }
    let io = owner(false, 2);
    let observed = Cell::new(IoTiming::default());
    let input = native_time(123.0, 456, 0.999);
    let output = native_time(789.0, 999, 1.001);
    let now = callback::Timestamp {
        host: 600,
        flags: 2,
        ..Default::default()
    };
    let before = unsafe { latency_snapshot(&raw const io, false) }.unwrap();
    let mut pcm = [0.0; 128];
    let mut list = buffers(&mut pcm, 2, 64, false);
    // SAFETY: disconnected exclusive fixture, complete PCM and native timestamps.
    unsafe {
        mr_device_io_render_hook(
            ptr::from_ref(&io).cast_mut(),
            ptr::from_ref(&observed).cast_mut().cast(),
            Some(timing_hook),
        );
    }
    let allocated = allocation_counter::measure(|| unsafe {
        mr_device_io_process(
            0,
            (&raw const now).cast(),
            ptr::null(),
            (&raw const input).cast(),
            (&raw mut list).cast(),
            (&raw const output).cast(),
            ptr::from_ref(&io).cast_mut().cast(),
        );
    });
    assert_eq!(allocated.count_total, 0);
    assert_eq!(
        observed.get(),
        IoTiming {
            now: IoTimestamp {
                host_time: 600,
                flags: 2,
                ..IoTimestamp::default()
            },
            input: IoTimestamp {
                sample_time: 123.0,
                host_time: 456,
                rate_scalar: 0.999,
                flags: 7
            },
            output: IoTimestamp {
                sample_time: 789.0,
                host_time: 999,
                rate_scalar: 1.001,
                flags: 7
            }
        }
    );
    let after = unsafe { latency_snapshot(&raw const io, false) }.unwrap();
    assert_eq!(
        after.acquisition.mean_since(before.acquisition),
        Some(144.0)
    );
    assert_eq!(
        after.presentation.mean_since(before.presentation),
        Some(399.0)
    );
    process(&io, &mut list);
    assert_eq!(observed.get(), IoTiming::default());
}
