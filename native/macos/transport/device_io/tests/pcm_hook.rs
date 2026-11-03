//! The native admission/transfer/conversion seam, without a device or worker thread.
use super::*;

struct Probe {
    io: *const DeviceIo,
    watch_invalid: Arc<AtomicBool>,
    phases: u32,
    invalidate: Option<PcmPhase>,
    scratch: [f32; 64],
}

unsafe extern "C" fn hook(context: *mut c_void, phase: u32) -> bool {
    let phase = PcmPhase::from_raw(phase).expect("native callback uses a known phase");
    // SAFETY: the fixture retains this exclusive probe until all callbacks end.
    let probe = unsafe { &mut *context.cast::<Probe>() };
    // SAFETY: the probe's owner remains alive; hooks never borrow Callback/scratch.
    let io = unsafe { &*probe.io };
    probe.phases = probe.phases * 10 + phase as u32 + 1;
    if probe.invalidate == Some(phase) {
        probe.watch_invalid.store(true, Release);
        return false;
    }
    // SAFETY: this hook owns only the bridge-side ring cursors; these fixed
    // stereo blocks are disjoint from native PCM and callback conversion scratch.
    unsafe {
        match (io.selection.input, phase) {
            (true, PcmPhase::Prepare) => assert_eq!(io.native_ring.stats().queued, 0),
            (true, PcmPhase::Captured) => {
                assert_eq!(queues::bridge_read(io, probe.scratch.as_mut_ptr(), 32), 32);
                assert_eq!(queues::bridge_write(io, probe.scratch.as_ptr(), 32), 32);
            }
            (false, PcmPhase::Prepare) => {
                assert_eq!(queues::bridge_write(io, [0.5; 64].as_ptr(), 32), 32);
            }
            (false, PcmPhase::Captured) => panic!("output received capture completion"),
        }
    }
    true
}

fn install(io: &DeviceIo, probe: &mut Probe) {
    // SAFETY: an unpublished fixture has no registered or concurrent callback;
    // the probe remains live through every synchronous invocation below.
    unsafe {
        mr_device_io_pcm_hook(
            ptr::from_ref(io).cast_mut(),
            ptr::from_mut(probe).cast(),
            hook,
        );
    }
}

fn probe(io: &DeviceIo, invalidate: Option<PcmPhase>) -> Probe {
    // SAFETY: disconnected setup copies this shared atomic before any callback runs.
    let watch_invalid = unsafe { &*io.callback.get() }.watch_invalid.clone();
    Probe {
        io,
        watch_invalid,
        phases: 0,
        invalidate,
        scratch: [0.0; 64],
    }
}

#[test]
fn capture_is_converted_in_its_callback_and_output_is_prepared_before_consumption() {
    assert_eq!(PcmPhase::from_raw(u32::MAX), None);
    for input in [true, false] {
        let io = owner(input, 2);
        let mut probe = probe(&io, None);
        install(&io, &mut probe);
        let mut pcm = [0.25; 64];
        let mut list = buffers(&mut pcm, 2, 32, false);
        let mut read = [0.0; 64];
        // Establish graph freshness before the input's first publication.
        if input {
            unsafe { queues::graph_read(&io, read.as_mut_ptr(), 32, 1, (0, None)) };
        }
        let allocated = allocation_counter::measure(|| process(&io, &mut list));
        assert_eq!(allocated.count_total, 0);
        assert_eq!(probe.phases, if input { 12 } else { 1 });
        assert_eq!(io.native_ring.stats().queued, 0);
        assert_eq!(io.progress.native_frames.load(Acquire), 32);
        if input {
            assert_eq!(
                unsafe { queues::graph_read(&io, read.as_mut_ptr(), 32, 1, (0, None)) }.frames,
                32
            );
            assert_pcm(&read, &[0.25; 64]);
        } else {
            assert_pcm(&pcm, &[0.5; 64]);
        }
    }
}

#[test]
fn invalidation_during_either_input_stage_blocks_further_native_admission() {
    for phase in [PcmPhase::Prepare, PcmPhase::Captured] {
        let io = owner(true, 2);
        let mut probe = probe(&io, Some(phase));
        install(&io, &mut probe);
        let mut pcm = [0.25; 64];
        let mut list = buffers(&mut pcm, 2, 32, false);
        let mut read = [9.0; 64];
        // Seed actual prior-epoch PCM so the graph gate has something to block.
        unsafe {
            assert!(!mr_device_io_invalid(ptr::from_ref(&io).cast_mut()));
            queues::graph_read(&io, read.as_mut_ptr(), 32, 1, (0, None));
            assert_eq!(queues::bridge_write(&io, [0.75; 64].as_ptr(), 32), 32);
        }
        read.fill(9.0);
        let allocated = allocation_counter::measure(|| process(&io, &mut list));
        assert_eq!(allocated.count_total, 0);
        assert_eq!(
            probe.phases,
            if phase == PcmPhase::Prepare { 1 } else { 12 }
        );
        assert!(io.progress.invalid.load(Acquire));
        assert!(unsafe { mr_device_io_invalid(ptr::from_ref(&io).cast_mut()) });
        assert_eq!(
            unsafe { queues::graph_read(&io, read.as_mut_ptr(), 32, 1, (0, None)) },
            GraphRead::default()
        );
        assert_pcm(&read, &[9.0; 64]);
        assert_eq!(io.graph_ring.stats().queued, 32);
        assert_eq!(io.progress.native_frames.load(Acquire), 0);
        let queued = io.native_ring.stats().queued;
        assert_eq!(queued, if phase == PcmPhase::Prepare { 0 } else { 32 });
        // The stable latch remains withdrawn even if a callback-local latch
        // were cleared; only acknowledged connection preparation may recover.
        probe.watch_invalid.store(false, Release);
        process(&io, &mut list);
        assert_eq!(io.native_ring.stats().queued, queued);
        assert_eq!(io.progress.native_frames.load(Acquire), 0);
    }
}

#[test]
fn direct_rejection_after_invalidation_never_pumps_the_pcm_fallback_or_accepts_graph_output() {
    unsafe extern "C" fn reject(
        context: *mut c_void,
        _: *const c_void,
        _: *mut c_void,
        _: u32,
        _: *const IoTiming,
    ) -> bool {
        // SAFETY: the synchronous fixture retains this same hook probe.
        unsafe { &*context.cast::<Probe>() }
            .watch_invalid
            .store(true, Release);
        false
    }
    let io = owner(false, 2);
    let mut probe = probe(&io, None);
    install(&io, &mut probe);
    let mut pcm = [0.25; 64];
    let mut list = buffers(&mut pcm, 2, 32, false);
    // SAFETY: unpublished fixture owns hook setup and serialized graph producer.
    unsafe {
        let callback = &mut *io.callback.get();
        callback.hook = Some(reject);
        callback.context = ptr::from_mut(&mut probe).cast();
        assert_eq!(queues::graph_write(&io, pcm.as_ptr(), 32, 0), 32);
    }
    let allocated = allocation_counter::measure(|| {
        process(&io, &mut list);
        // SAFETY: same serialized graph producer and complete disjoint PCM.
        assert_eq!(unsafe { queues::graph_write(&io, pcm.as_ptr(), 32, 0) }, 0);
    });
    assert_eq!(allocated.count_total, 0);
    assert_eq!(probe.phases, 0);
    assert!(io.progress.invalid.load(Acquire));
    assert_eq!(io.graph_ring.stats().queued, 32);
    assert_eq!(io.progress.ticks.load(Acquire), 0);
    assert_pcm(&pcm, &[0.0; 64]);
    assert!(unsafe { mr_device_io_invalid(ptr::null_mut()) });
}
