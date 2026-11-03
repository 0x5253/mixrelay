use super::*;
use std::alloc::alloc;

struct Owner(*mut AudioRing);

impl Owner {
    fn new(capacity: usize, channels: u32) -> Self {
        let ring = mr_audio_ring_create(capacity, channels);
        assert!(!ring.is_null());
        Self(ring)
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        // SAFETY: tests retain one exclusive owner, with every native call finished.
        unsafe { mr_audio_ring_destroy(self.0) };
    }
}

#[test]
fn both_allocation_failures_return_null_and_release_prepared_pcm() {
    // SAFETY: first allocator deliberately fails; owner allocator must not run.
    let failed = unsafe { create(4, 2, |_| None, |_| panic!("no PCM prepared")) };
    assert!(failed.is_null());
    let measured = allocation_counter::measure(|| {
        // SAFETY: second allocator fails after actual fallible vector allocation.
        assert!(unsafe { create(4, 2, allocate_samples, |_| None) }.is_null());
    });
    assert_eq!(measured.count_total, 2); // PCM and matching coordinate storage both unwind.
    assert_eq!(measured.count_current, 0);
    assert_eq!(measured.bytes_current, 0);
    assert!(allocate_samples(usize::MAX).is_none());
    // SAFETY: standard allocator returns correctly sized/aligned owner storage.
    let owner =
        Owner(unsafe { create(4, 2, allocate_samples, |layout| NonNull::new(alloc(layout))) });
    assert!(!owner.0.is_null());
}

#[test]
fn invalid_geometry_is_rejected_before_allocating() {
    for (capacity, channels) in [(0, 2), (3, 2), (1 << 21, 2), (8, 0), (8, 65)] {
        // SAFETY: rejected geometry must never reach either allocator callback.
        assert!(
            unsafe {
                create(
                    capacity,
                    channels,
                    |_| panic!("PCM allocation"),
                    |_| panic!("owner allocation"),
                )
            }
            .is_null()
        );
    }
}

#[test]
fn planar_wrap_partial_reads_and_pcm_bits_cover_every_channel_width() {
    for channels in 1..=64_u32 {
        let owner = Owner::new(4, channels);
        let width = usize::try_from(channels).unwrap();
        let planes: Vec<_> = (0..width)
            .map(|channel| {
                [
                    f32::from_bits(0x7fc0_0000 + u32::try_from(channel).unwrap()),
                    -0.0,
                    f32::INFINITY,
                ]
            })
            .collect();
        let input: Vec<_> = planes.iter().map(|plane| plane.as_ptr()).collect();
        let mut output = vec![9.0_f32; 5 * width];
        // SAFETY: single producer/consumer calls, sized planes/output disjoint from ring.
        unsafe {
            assert_eq!(mr_audio_ring_write_planar(owner.0, input.as_ptr(), 3), 3);
            assert_eq!(
                mr_audio_ring_read_interleaved(owner.0, output.as_mut_ptr(), 2),
                2
            );
            assert_eq!(mr_audio_ring_write_planar(owner.0, input.as_ptr(), 3), 3);
            assert_eq!(
                mr_audio_ring_read_interleaved(owner.0, output.as_mut_ptr(), 5),
                4
            );
        }
        for channel in 0..width {
            assert_eq!(output[channel].to_bits(), f32::INFINITY.to_bits());
            for frame in 0..3 {
                assert_eq!(
                    output[(frame + 1) * width + channel].to_bits(),
                    planes[channel][frame].to_bits()
                );
            }
            assert_eq!(output[4 * width + channel].to_bits(), 9.0_f32.to_bits());
        }
    }
}

#[test]
fn cursor_and_dropped_rollover_do_not_panic_or_replay_samples() {
    let owner = Owner::new(4, 1);
    // SAFETY: test exclusively owns the ring while seeding near-rollover atomics.
    let ring = unsafe { &*owner.0 };
    ring.read.store(u64::MAX - 1, Ordering::Relaxed);
    ring.written.store(u64::MAX - 1, Ordering::Relaxed);
    ring.dropped.store(u64::MAX, Ordering::Relaxed);
    let input = [1.0, 2.0, 3.0, 4.0];
    let mut output = [0.0_f32; 4];
    // SAFETY: no concurrent accesses, complete sized input/output blocks.
    unsafe {
        assert_eq!(
            mr_audio_ring_write_interleaved(owner.0, input.as_ptr(), 4),
            4
        );
        assert_eq!(
            mr_audio_ring_write_interleaved(owner.0, input.as_ptr(), 1),
            0
        );
        assert_eq!(mr_audio_ring_dropped_frames(owner.0), 0);
        assert_eq!(
            mr_audio_ring_read_interleaved(owner.0, output.as_mut_ptr(), 4),
            4
        );
    }
    assert_eq!(input.map(f32::to_bits), output.map(f32::to_bits));
    assert_eq!(ring.read.load(Ordering::Relaxed), 2);
}

#[test]
fn oversized_and_invalid_inputs_do_not_read_unaccepted_pcm_or_double_count() {
    let owner = Owner::new(1, 2);
    let input = [1.0_f32, 2.0];
    let planes = [input.as_ptr(), ptr::null()];
    let mut output = [5.0_f32; 2];
    // SAFETY: oversized blocks must be rejected before input pointer dereference;
    // accepted input and output contain the one complete stereo frame.
    unsafe {
        assert_eq!(
            mr_audio_ring_write_interleaved(owner.0, input.as_ptr(), usize::MAX),
            0
        );
        assert_eq!(mr_audio_ring_dropped_frames(owner.0), u64::MAX);
        assert_eq!(mr_audio_ring_write_planar(owner.0, planes.as_ptr(), 1), 0);
        assert_eq!(mr_audio_ring_dropped_frames(owner.0), 0);
        assert_eq!(
            mr_audio_ring_write_interleaved(owner.0, input.as_ptr(), 1),
            1
        );
        assert_eq!(mr_audio_ring_write_planar(owner.0, planes.as_ptr(), 1), 0);
        assert_eq!(mr_audio_ring_dropped_frames(owner.0), 1);
        assert_eq!(
            mr_audio_ring_read_interleaved(owner.0, output.as_mut_ptr(), usize::MAX),
            1
        );
    }
    assert_eq!(input.map(f32::to_bits), output.map(f32::to_bits));
}

#[test]
fn statistics_bound_a_nontransactional_cursor_observation_without_moving_cursors() {
    let owner = Owner::new(4, 2);
    // SAFETY: exclusive owner seeds the cursor difference that a stat reader may
    // observe if consumption and publication progress between its two loads.
    let ring = unsafe { &*owner.0 };
    ring.read.store(10, Ordering::Relaxed);
    ring.written.store(18, Ordering::Relaxed);
    // SAFETY: only stats are read; this deliberately non-SPSC state is never used for PCM.
    assert_eq!(unsafe { mr_audio_ring_available_frames(owner.0) }, 4);
    assert_eq!(ring.read.load(Ordering::Relaxed), 10);
    assert_eq!(ring.written.load(Ordering::Relaxed), 18);
}

#[test]
fn callback_io_is_allocation_free_and_destroy_releases_pcm_coordinates_and_owner() {
    let lifecycle = allocation_counter::measure(|| {
        let owner = Owner::new(1024, 2);
        let input = [0.5_f32; 512];
        let mut output = [0.0_f32; 512];
        let callback = allocation_counter::measure(|| {
            for _ in 0..10_000 {
                // SAFETY: sequential SPSC calls with complete non-overlapping buffers.
                unsafe {
                    assert_eq!(
                        mr_audio_ring_write_interleaved(owner.0, input.as_ptr(), 256),
                        256
                    );
                    assert_eq!(
                        mr_audio_ring_read_interleaved(owner.0, output.as_mut_ptr(), 256),
                        256
                    );
                }
            }
        });
        assert_eq!(callback.count_total, 0);
        assert_eq!(callback.count_current, 0);
    });
    assert_eq!(lifecycle.count_total, 3);
    assert_eq!(lifecycle.count_current, 0);
    assert_eq!(lifecycle.bytes_current, 0);
}
