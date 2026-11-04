use super::*;

#[test]
fn latency_fifo_taps_time_each_role_and_invalidate_discarded_windows() {
    let io = owner(true, 1);
    let mut pcm = [0.25; 64];
    let mut list = buffers(&mut pcm, 1, 64, false);
    let mut read = [0.0; 64];
    // Establish freshness before writing graph PCM.
    unsafe { queues::graph_read(&io, read.as_mut_ptr(), 1, 7, (0, None)) };
    let before = unsafe { latency_snapshot(&raw const io, false) }.unwrap();
    // Fixture callbacks carry no `now`; set the host time each one would store.
    let callback = |host: u64| io.progress.callback_host.store(host, Release);
    let allocations = allocation_counter::measure(|| {
        callback(1_000);
        process(&io, &mut list);
        callback(1_256);
        unsafe {
            assert_eq!(queues::bridge_read(&io, read.as_mut_ptr(), 32), 32);
            assert_eq!(queues::bridge_write(&io, pcm.as_ptr(), 64), 64);
            assert_eq!(
                queues::graph_read(&io, read.as_mut_ptr(), 32, 7, (1_300, None)).frames,
                32
            );
            assert_eq!(
                queues::graph_read(&io, read.as_mut_ptr(), 64, 7, (1_310, None)).frames,
                0
            );
            assert_eq!(
                queues::graph_read(&io, read.as_mut_ptr(), 32, 7, (1_400, None)).frames,
                32
            );
        }
    });
    assert_eq!(allocations.count_total, 0);
    let after = unsafe { latency_snapshot(&raw const io, false) }.unwrap();
    // The whole native block was written at once and waited a full callback, not
    // half the fill the bridge saw; graph frames waited 44 and 144 ticks.
    assert_eq!(
        after.native_queue.residence_since(before.native_queue),
        Some(256.0)
    );
    assert_eq!(
        after.graph_queue.residence_since(before.graph_queue),
        Some(94.0)
    );
    assert_eq!(after.faults, before.faults + 1);
    callback(1_512);
    process(&io, &mut list);
    // SAFETY: this fixture is the native ring's serialized SRC consumer.
    unsafe { queues::bridge_discard(&io) };
    callback(1_768);
    process(&io, &mut list);
    callback(2_024);
    assert_eq!(
        unsafe { queues::bridge_read(&io, read.as_mut_ptr(), 32) },
        32
    );
    let discarded = unsafe { latency_snapshot(&raw const io, false) }.unwrap();
    assert_eq!(
        discarded.native_queue.residence_since(after.native_queue),
        None
    );
    assert_eq!(
        unsafe { queues::bridge_read(&io, read.as_mut_ptr(), 32) },
        32
    );
    let next = unsafe { latency_snapshot(&raw const io, false) }.unwrap();
    assert_eq!(
        next.native_queue.residence_since(discarded.native_queue),
        Some(256.0)
    );
}

#[test]
fn starvation_snapshot_distinguishes_the_graph_consumer_from_native_input_polling() {
    let io = owner(true, 1);
    let pointer = ptr::from_ref(&io).cast_mut();
    let mut pcm = [0.0; 32];
    // SAFETY: this unpublished fixture serializes both queue roles; all buffers
    // own a complete 32-frame mono block throughout each bounded transfer.
    unsafe {
        queues::graph_read(&io, pcm.as_mut_ptr(), 32, 1, (0, None));
        assert_eq!(queues::bridge_write(&io, [0.25; 32].as_ptr(), 32), 32);
        assert_eq!(
            queues::graph_read(&io, pcm.as_mut_ptr(), 32, 1, (0, None)).frames,
            32
        );
        assert_eq!(
            queues::graph_read(&io, pcm.as_mut_ptr(), 32, 1, (0, None)).frames,
            0
        );
        assert_eq!(mr_device_io_starvations(pointer, true), 1);
        assert_eq!(mr_device_io_starvations(pointer, false), 0);
        assert_eq!(mr_device_io_starvations(ptr::null_mut(), true), 0);
    }
}

#[test]
fn graph_read_admits_initial_epoch_once_without_hiding_later_shortages() {
    let io = owner(true, 1);
    let pointer = ptr::from_ref(&io).cast_mut();
    let mut pcm = [9.0; 32];
    // SAFETY: unpublished owner, serialized queue roles, complete disjoint PCM.
    // Rejected requests never touch PCM, including their oversized geometry.
    unsafe {
        assert_eq!(
            mr_device_io_graph_read(ptr::null_mut(), pcm.as_mut_ptr(), 32, 1, 0),
            GraphRead::default()
        );
        assert_eq!(
            mr_device_io_graph_read(pointer, pcm.as_mut_ptr(), 0, 1, 0),
            GraphRead::default()
        );
        assert_eq!(
            mr_device_io_graph_read(pointer, pcm.as_mut_ptr(), MAX_FRAMES + 1, 1, 0),
            GraphRead::default()
        );
        assert_eq!(queues::bridge_write(&io, [0.25; 32].as_ptr(), 32), 32);
        assert_eq!(
            mr_device_io_graph_read(pointer, pcm.as_mut_ptr(), 32, 0, 0),
            GraphRead {
                frames: 0,
                initial_epoch: true,
                discontinuity: false,
            }
        );
        assert!(pcm.iter().all(|sample| *sample == 0.0));
        assert_eq!(
            mr_device_io_graph_read(pointer, pcm.as_mut_ptr(), 32, 0, 0),
            GraphRead::default()
        );
        assert_eq!(queues::bridge_write(&io, [0.75; 32].as_ptr(), 32), 32);
        assert_eq!(
            mr_device_io_graph_read(pointer, pcm.as_mut_ptr(), 32, 0, 0),
            GraphRead {
                frames: 32,
                initial_epoch: false,
                discontinuity: false,
            }
        );
        assert_eq!(pcm.map(f32::to_bits), [0.75_f32.to_bits(); 32]);
        assert_eq!(
            mr_device_io_graph_read(pointer, pcm.as_mut_ptr(), 32, 0, 0),
            GraphRead::default()
        );
        assert_eq!(mr_device_io_starvations(pointer, true), 1);
        assert_eq!(queues::bridge_write(&io, [0.5; 32].as_ptr(), 32), 32);
        assert_eq!(
            mr_device_io_graph_read(pointer, pcm.as_mut_ptr(), 32, 1, 0),
            GraphRead {
                discontinuity: true,
                ..GraphRead::default()
            }
        );
        assert!(pcm.iter().all(|sample| *sample == 0.0));
        assert_eq!(
            mr_device_io_graph_read(pointer, pcm.as_mut_ptr(), 32, 1, 0),
            GraphRead::default(),
            "the same recovery is reported once"
        );
        assert_eq!(queues::bridge_write(&io, [0.875; 32].as_ptr(), 32), 32);
        let read = mr_device_io_graph_read(pointer, pcm.as_mut_ptr(), 32, 1, 0);
        assert_eq!(read.frames, 32);
        assert!(!read.initial_epoch && !read.discontinuity);
        assert_eq!(pcm.map(f32::to_bits), [0.875_f32.to_bits(); 32]);
    }
}
