use super::*;
use crate::macos_devices::tests::synthetic_device;
use mixrelay_macos_transport::{audio_buffer::AudioBuffer, device_io::mr_device_io_process};

#[repr(C)]
struct Buffer {
    count: u32,
    buffers: [AudioBuffer; 1],
}

fn tick(context: &WorkerContext, samples: &mut [f32; 32]) {
    let mut buffer = Buffer {
        count: 1,
        buffers: [AudioBuffer {
            channels: 1,
            byte_size: 128,
            data: samples.as_mut_ptr(),
        }],
    };
    // SAFETY: no OS registration exists; this fixture is the sole native caller
    // with a complete mono ABL, disjoint PCM, and a retained callback owner.
    unsafe {
        mr_device_io_process(
            0,
            ptr::null(),
            if context.input {
                ptr::from_ref(&buffer).cast()
            } else {
                ptr::null()
            },
            ptr::null(),
            if context.input {
                ptr::null_mut()
            } else {
                ptr::from_mut(&mut buffer).cast()
            },
            ptr::null(),
            context.native.pointer.as_ptr(),
        );
    }
}

fn queued(context: &WorkerContext, graph: bool) -> u32 {
    // SAFETY: the fixture retains its native owner; this reads queue atomics only.
    unsafe { ffi::mr_device_io_queued(context.native.pointer.as_ptr(), graph) }
}

#[test]
fn aligned_capture_converts_the_current_packet_once_in_the_same_native_callback() {
    let (device, context) = synthetic_device(1, 48_000, 48_000, true);
    context.clock_aligned.store(true, Ordering::Release);
    let mut owner = CallbackBridge::new(&context, 48_000).unwrap();
    // The graph has already admitted this stream's freshness epoch; startup
    // admission is separate from the native capture/conversion ordering here.
    context
        .epoch
        .store(owner.state.get_mut().bridge.epoch, Ordering::Release);
    let mut graph = [0.0; 32];
    device.read(&mut graph, 0);
    let allocated = allocation_counter::measure(|| {
        for value in [0.25_f32, 0.5, -0.25] {
            tick(&context, &mut [value; 32]);
            assert_eq!(device.read(&mut graph, 0).frames, 32);
            assert_eq!(graph.map(f32::to_bits), [value.to_bits(); 32]);
            assert_eq!(queued(&context, false), 0);
        }
    });
    assert_eq!(allocated.count_total, 0);
    assert_eq!(context.native.pcm_metrics.work_calls(), 3);
    assert!(context.native.pcm.failure().is_none());
}

#[test]
fn native_callback_invalidation_blocks_ready_and_queued_input_before_any_control_poll() {
    let (device, context) = synthetic_device(1, 48_000, 48_000, true);
    let mut graph = [9.0; 32];
    device.read(&mut graph, 0); // Admit the current graph consumer epoch.
    // SAFETY: this fixture is the only producer, with complete immutable PCM.
    assert_eq!(
        unsafe {
            ffi::mr_device_io_bridge_write(context.native.pointer.as_ptr(), [0.25; 32].as_ptr(), 32)
        },
        32
    );
    assert!(device.ready());
    let malformed = Buffer {
        count: 1,
        buffers: [AudioBuffer {
            channels: 1,
            byte_size: 3, // Not even one complete Float32 native frame.
            data: graph.as_mut_ptr(),
        }],
    };
    let allocated = allocation_counter::measure(|| {
        // SAFETY: complete retained ABL metadata; malformed frame geometry is
        // rejected before accessing PCM. No control worker or callback races.
        unsafe {
            mr_device_io_process(
                0,
                ptr::null(),
                ptr::from_ref(&malformed).cast(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null(),
                context.native.pointer.as_ptr(),
            );
        }
        assert!(!device.ready());
        let lost = device.read(&mut graph, 0);
        assert_eq!(lost.frames, 0);
        assert!(lost.discontinuity);
        assert!(!device.read(&mut graph, 0).discontinuity);
        assert_eq!(graph.map(f32::to_bits), [0.0_f32.to_bits(); 32]);
    });
    assert_eq!(allocated.count_total, 0);
    assert!(!context.failed.load(Ordering::Acquire));
    assert_eq!(context.ready_rate.load(Ordering::Acquire), 48_000);
    assert_eq!(queued(&context, true), 32, "no stale PCM was consumed");
}

#[test]
fn input_off_and_unprepared_formats_never_accumulate_or_overflow_capture_pcm() {
    for unprepared in [false, true] {
        let (device, context) = synthetic_device(1, 48_000, 48_000, true);
        let _owner = CallbackBridge::new(&context, 48_000).unwrap();
        if unprepared {
            context.graph_rate.store(96_000, Ordering::Release);
        } else {
            device.set_active(false);
        }
        let allocated = allocation_counter::measure(|| {
            // More frames than the complete native ring capacity: a wrong
            // capture-before-admission order would accumulate and then drop PCM.
            for _ in 0..600 {
                tick(&context, &mut [0.25; 32]);
            }
        });
        assert_eq!(allocated.count_total, 0);
        assert_eq!(context.native.pcm_metrics.work_calls(), 600);
        for graph in [false, true] {
            assert_eq!(queued(&context, graph), 0);
        }
        // SAFETY: retained fixture owner, atomic-only overflow observation.
        assert_eq!(
            unsafe { ffi::mr_device_io_overflows(context.native.pointer.as_ptr()) },
            0
        );
        assert!(context.native.pcm.failure().is_none());
    }
}

#[test]
fn replaced_converter_requires_fresh_capture_admission_and_unknown_phases_are_rejected() {
    let (_device, context) = synthetic_device(1, 48_000, 48_000, true);
    context.clock_aligned.store(true, Ordering::Release);
    let mut owner = CallbackBridge::new(&context, 48_000).unwrap();
    let slot = ptr::from_ref(context.native.pcm.as_ref()).cast_mut().cast();
    // SAFETY: this fixture is the only native caller; raw phase validation
    // rejects an unknown value before reading any native or PCM state.
    assert!(!unsafe { dispatch(slot, u32::MAX) });
    // SAFETY: the retained slot is exclusively driven by this synchronous fixture.
    assert!(unsafe { dispatch(slot, PcmPhase::Prepare as u32) });
    owner.replace(DeviceBridge::prepare(1, true, 48_000, 48_000, true).unwrap());
    // SAFETY: the old stage completed before replacement; this is its late completion.
    assert!(!unsafe { dispatch(slot, PcmPhase::Captured as u32) });
    assert!(context.native.pcm.failure().is_none());
    assert_eq!(context.ready_rate.load(Ordering::Acquire), 0);
    tick(&context, &mut [0.5; 32]);
    assert_eq!(context.ready_rate.load(Ordering::Acquire), 48_000);
    assert!(context.bridge_ready.load(Ordering::Acquire));
    assert_eq!(queued(&context, true), 32);
}

#[test]
fn capture_sanitizes_before_copy_or_sinc_and_reports_faults_once_without_stopping() {
    for aligned in [false, true] {
        let (device, context) = synthetic_device(1, 48_000, 48_000, true);
        context.clock_aligned.store(aligned, Ordering::Release);
        let mut owner = CallbackBridge::new(&context, 48_000).unwrap();
        context
            .epoch
            .store(owner.state.get_mut().bridge.epoch, Ordering::Release);
        let mut graph = [0.0; 32];
        device.read(&mut graph, 0);
        let mut malformed = [0.25; 32];
        malformed[..3].copy_from_slice(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY]);
        let allocated = allocation_counter::measure(|| {
            tick(&context, &mut malformed);
            let read = device.read(&mut graph, 0);
            assert_eq!(read.source_sample_faults, 3);
            if aligned {
                assert_eq!(read.frames, 32);
                assert!(graph[..3].iter().all(|sample| sample.to_bits() == 0));
                assert!(
                    graph[3..]
                        .iter()
                        .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
                );
            }
            for cycle in 0..64 {
                tick(&context, &mut [0.5; 32]);
                let read = device.read(&mut graph, 0);
                assert_eq!(read.source_sample_faults, 0);
                if cycle > 32 {
                    assert_eq!(read.frames, 32);
                    assert!(graph.iter().all(|sample| (*sample - 0.5).abs() < 0.001));
                }
            }
        });
        assert_eq!(allocated.count_total, 0);
        assert!(context.native.pcm.failure().is_none());
        assert!(context.bridge_ready.load(Ordering::Acquire));
        assert_eq!(context.native.pcm_metrics.work_calls(), 65);
    }
}

#[test]
fn conversion_failure_after_capture_withdraws_old_pcm_until_a_fresh_owner_recovers() {
    use mixrelay_audio_io::clock_bridge::{
        AsyncClockBridge, BridgeConfig, ClockRelation, Discontinuity, FeedbackQueue,
    };
    let (device, context) = synthetic_device(1, 48_000, 48_000, true);
    context.clock_aligned.store(true, Ordering::Release);
    let mut owner = CallbackBridge::new(&context, 48_000).unwrap();
    let mut consumed = [0.0; 16];
    tick(&context, &mut [0.25; 32]);
    assert_eq!(device.read(&mut consumed, 0).frames, 0); // Initial freshness admission.
    tick(&context, &mut [0.25; 32]);
    assert_eq!(device.read(&mut consumed, 0).frames, 16);
    assert_eq!(queued(&context, true), 16); // Leave old PCM below the pump target.
    let before = context.epoch.load(Ordering::Acquire);
    // Inject a converter that rejects the callback's output quantum. This is an
    // explicit bounded failure after native PCM was admitted, without a real HAL.
    let state = owner.state.get_mut();
    state.bridge.bridge = AsyncClockBridge::new(BridgeConfig {
        channels: 1,
        input_rate: 48_000,
        output_rate: 48_000,
        max_input_frames: 256,
        max_output_frames: 1,
        capacity_frames: 256,
        priming_frames: 1,
        clock_relation: ClockRelation::ProvenSameClock,
        feedback_queue: FeedbackQueue::Input,
        max_correction_ppm: 1_000.0,
    })
    .unwrap();
    state
        .bridge
        .bridge
        .reset(state.bridge.epoch, Discontinuity::StreamRestart);
    let mut graph = [9.0; 32];
    let allocated = allocation_counter::measure(|| {
        tick(&context, &mut [0.5; 32]);
        assert_eq!(context.ready_rate.load(Ordering::Acquire), 0);
        assert_ne!(context.epoch.load(Ordering::Acquire), before);
        assert_eq!(queued(&context, false), 0);
        assert_eq!(device.read(&mut graph, 0).frames, 0);
        assert_eq!(graph.map(f32::to_bits), [0.0_f32.to_bits(); 32]);
        tick(&context, &mut [0.75; 32]);
        assert_eq!(queued(&context, false), 0);
    });
    assert_eq!(allocated.count_total, 0);
    assert!(context.native.pcm.failure().is_some());
    drop(owner);
    let _recovered = CallbackBridge::new(&context, 48_000).unwrap();
    tick(&context, &mut [0.75; 32]);
    device.read(&mut graph, 0); // Freshness excludes every pre-recovery packet.
    tick(&context, &mut [0.75; 32]);
    assert_eq!(device.read(&mut graph, 0).frames, 32);
    assert_eq!(graph.map(f32::to_bits), [0.75_f32.to_bits(); 32]);
    assert!(context.native.pcm.failure().is_none());
}

#[test]
fn native_callback_alone_converts_mixed_rates_without_polling_or_rt_allocation() {
    for input in [true, false] {
        let (device, context) = synthetic_device(1, 48_000, 96_000, input);
        let owner = CallbackBridge::new(&context, 96_000).unwrap();
        let mut native = [0.25; 32];
        let mut graph = [0.0; 32];
        device.read(&mut graph, 0);
        let mut missing = 0;
        let allocation = allocation_counter::measure(|| {
            for cycle in 0..30_000 {
                if !input && cycle % 2 == 0 {
                    assert_eq!(device.write(&[0.25; 32], 0).frames, 32);
                }
                tick(&context, &mut native);
                if input && cycle % 2 == 0 {
                    device.read(&mut graph, 0);
                    if cycle > 300 {
                        missing += graph.iter().filter(|x| (**x - 0.25).abs() > 0.001).count();
                    }
                } else if !input && cycle > 300 {
                    missing += native.iter().filter(|x| (**x - 0.25).abs() > 0.001).count();
                }
            }
        });
        assert_eq!(missing, 0, "native callback continuity input={input}");
        assert_eq!(allocation.count_total, 0);
        assert!(context.native.pcm.failure().is_none());
        if !input {
            // SAFETY: the fixture retains native storage; counter access is atomic.
            assert_eq!(
                unsafe { ffi::mr_device_io_starvations(context.native.pointer.as_ptr(), false) },
                0
            );
        }
        drop(owner);
        assert!(context.native.pcm.state.load(Ordering::Acquire).is_null());
    }
}

#[test]
fn off_on_is_acknowledged_by_callback_and_dropped_owner_cannot_run_again() {
    let (device, context) = synthetic_device(1, 48_000, 48_000, false);
    context.clock_aligned.store(true, Ordering::Release);
    let owner = CallbackBridge::new(&context, 48_000).unwrap();
    let mut native = [0.0; 32];
    tick(&context, &mut native);
    device.write(&[0.25; 32], 0);
    tick(&context, &mut native);
    assert_eq!(native.map(f32::to_bits), [0.25_f32.to_bits(); 32]);
    device.set_active(false);
    tick(&context, &mut native);
    assert_eq!(native.map(f32::to_bits), [0.0_f32.to_bits(); 32]);
    device.set_active(true);
    tick(&context, &mut native);
    device.write(&[0.75; 32], 0);
    tick(&context, &mut native);
    assert_eq!(native.map(f32::to_bits), [0.75_f32.to_bits(); 32]);
    drop(owner);
    tick(&context, &mut native);
    assert_eq!(native.map(f32::to_bits), [0.0_f32.to_bits(); 32]);
}

#[test]
fn reader_drain_precedes_replacement_or_freeing_callback_state() {
    let (_device, context) = synthetic_device(1, 48_000, 48_000, false);
    let owner = CallbackBridge::new(&context, 48_000).unwrap();
    let slot = &context.native.pcm;
    slot.readers.store(1, Ordering::SeqCst);
    let drained = AtomicBool::new(false);
    thread::scope(|scope| {
        scope.spawn(|| {
            drop(owner);
            drained.store(true, Ordering::Release);
        });
        thread::sleep(Duration::from_millis(5));
        assert!(!drained.load(Ordering::Acquire));
        slot.readers.store(0, Ordering::SeqCst);
    });
    assert!(slot.state.load(Ordering::SeqCst).is_null());
}

#[test]
fn resumed_output_primes_real_converted_pcm_before_admitting_native_reads() {
    let (device, context) = synthetic_device(1, 48_000, 96_000, false);
    let _owner = CallbackBridge::new(&context, 96_000).unwrap();
    let mut native = [0.0; 32];
    let allocation = allocation_counter::measure(|| {
        for cycle in 0..3000 {
            if cycle % 300 == 100 {
                device.set_active(false);
            }
            if cycle % 300 == 200 {
                device.set_active(true);
            }
            if cycle % 2 == 0 {
                device.write(&[0.25; 32], 0);
            }
            tick(&context, &mut native);
        }
    });
    assert_eq!(allocation.count_total, 0);
    assert!(native.iter().all(|sample| (*sample - 0.25).abs() < 0.001));
    // SAFETY: retained synthetic owner, atomic snapshots of the native consumer.
    assert_eq!(
        unsafe { ffi::mr_device_io_starvations(context.native.pointer.as_ptr(), false) },
        0
    );
    assert_eq!(
        unsafe { ffi::mr_device_io_overflows(context.native.pointer.as_ptr()) },
        0
    );
}

#[test]
fn control_lock_cannot_block_pcm_and_format_replacement_preserves_owner() {
    let (_device, context) = synthetic_device(1, 48_000, 48_000, true);
    let mut owner = CallbackBridge::new(&context, 48_000).unwrap();
    thread::scope(|scope| {
        let guard = context.pending_bridge.lock().unwrap();
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        let callback_context = &context;
        scope.spawn(move || {
            tick(callback_context, &mut [0.25; 32]);
            sent.send(()).unwrap();
        });
        let completed = received.recv_timeout(Duration::from_secs(1)).is_ok();
        drop(guard);
        assert!(completed, "control lock blocked native PCM");
    });
    let pointer = context.native.pointer;
    context.graph_rate.store(96_000, Ordering::Release);
    tick(&context, &mut [0.25; 32]);
    assert_eq!(context.ready_rate.load(Ordering::Acquire), 0);
    owner.replace(DeviceBridge::prepare(1, true, 48_000, 96_000, false).unwrap());
    tick(&context, &mut [0.25; 32]);
    assert_eq!(context.ready_rate.load(Ordering::Acquire), 96_000);
    assert_eq!(context.native.pointer, pointer);
}
