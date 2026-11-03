use super::*;
use std::{
    ptr,
    sync::atomic::Ordering::{Relaxed, SeqCst},
};

#[test]
fn metadata_observation_retires_only_acknowledged_epochs_without_priming_pcm() {
    let mut ring = PcmRing::new(1, 8).unwrap();
    let mut output = [9.0; 4];
    let measured = allocation_counter::measure(|| {
        // SAFETY: retained initialized owner, serialized roles, complete disjoint PCM.
        unsafe {
            for _ in 0..2 {
                assert_eq!(mr_pcm_ring_write(&raw mut ring, [0.25; 4].as_ptr(), 4), 4);
            }
            mr_pcm_ring_restart(&raw mut ring);
            assert!(consumer_stats(&raw mut ring).is_none());
            assert_eq!(ring.stats().read, 0);
            mr_pcm_ring_write(&raw mut ring, ptr::null(), 0);
            let fresh = consumer_stats(&raw mut ring).unwrap();
            assert_eq!((fresh.read, fresh.written, fresh.queued), (8, 8, 0));
            assert_eq!(mr_pcm_ring_write(&raw mut ring, [0.75; 4].as_ptr(), 4), 4);
            assert_eq!(consumer_stats(&raw mut ring).unwrap().queued, 4);
            assert_eq!(mr_pcm_ring_read(&raw mut ring, output.as_mut_ptr(), 4), 0);
            assert_eq!(output.map(f32::to_bits), [0; 4]);
            assert_eq!(mr_pcm_ring_write(&raw mut ring, [0.75; 4].as_ptr(), 4), 4);
            assert_eq!(mr_pcm_ring_read(&raw mut ring, output.as_mut_ptr(), 4), 4);
            assert_eq!(output.map(f32::to_bits), [0.75_f32.to_bits(); 4]);
        }
    });
    assert_eq!(measured.count_total, 0);
}

#[test]
fn metadata_observer_only_sees_reserved_writes_and_protected_reads() {
    let mut ring = PcmRing::new(1, 8).unwrap();
    let mut metadata = [0; 8];
    let mut output = [0.0; 4];
    let measured = allocation_counter::measure(|| unsafe {
        // SAFETY: one synthetic retained ring, serialized roles, complete disjoint buffers.
        assert_eq!(
            write_observed(&raw mut ring, [0.5; 4].as_ptr(), 4, |cursor| {
                assert_eq!(cursor, 0);
                metadata[..4].fill(7);
            }),
            4
        );
        assert_eq!(
            write_observed(&raw mut ring, [0.5; 4].as_ptr(), 4, |cursor| {
                assert_eq!(cursor, 4);
                metadata[4..].fill(7);
            }),
            4
        );
        assert_eq!(
            write_observed(&raw mut ring, [0.75].as_ptr(), 1, |_| panic!(
                "full ring must not replace unread metadata"
            )),
            0
        );
        assert_eq!(
            read_available_observed(&raw mut ring, output.as_mut_ptr(), 4, |cursor| {
                assert_eq!(cursor, 0);
                assert_eq!(metadata, [7; 8]);
            }),
            4
        );
        assert_eq!(output.map(f32::to_bits), [0.5_f32.to_bits(); 4]);
    });
    assert_eq!(measured.count_total, 0);
}

#[test]
fn neutral_pcm_wraps_without_replaying_samples_or_allocating() {
    for channels in [1, 2, 32, 64] {
        let mut ring = PcmRing::new(channels, 7).unwrap();
        let input = vec![0.75; channels as usize * 2];
        let mut output = vec![1.0; input.len()];
        let measured = allocation_counter::measure(|| {
            // SAFETY: serialized producer/consumer, retained owner and complete
            // disjoint two-frame buffers; no callback outlives this scope.
            unsafe {
                for _ in 0..12 {
                    assert_eq!(mr_pcm_ring_write(&raw mut ring, input.as_ptr(), 2), 2);
                    assert_eq!(mr_pcm_ring_write_silence(&raw mut ring, 2), 2);
                    assert_eq!(mr_pcm_ring_read(&raw mut ring, output.as_mut_ptr(), 2), 2);
                    assert!(
                        output
                            .iter()
                            .zip(&input)
                            .all(|(actual, expected)| { actual.to_bits() == expected.to_bits() })
                    );
                    assert_eq!(mr_pcm_ring_read(&raw mut ring, output.as_mut_ptr(), 2), 2);
                    assert!(output.iter().all(|sample| sample.to_bits() == 0));
                }
            }
        });
        assert_eq!(measured.count_total, 0);
    }
}

#[test]
fn neutral_pcm_rejects_overflow_without_erasing_unread_audio_and_acknowledges_epoch() {
    let mut ring = PcmRing::new(2, 8).unwrap();
    let input = [0.5; 8];
    let mut output = [1.0; 8];
    // SAFETY: one retained initialized ring, sequential roles, complete buffers.
    unsafe {
        assert_eq!(mr_pcm_ring_write(&raw mut ring, input.as_ptr(), 4), 4);
        assert_eq!(mr_pcm_ring_write_silence(&raw mut ring, 4), 4);
        assert_eq!(mr_pcm_ring_write_silence(&raw mut ring, 1), 0);
        assert_eq!(mr_pcm_ring_read(&raw mut ring, output.as_mut_ptr(), 4), 4);
        assert_eq!(output.map(f32::to_bits), input.map(f32::to_bits));
        assert_eq!(mr_pcm_ring_read(&raw mut ring, output.as_mut_ptr(), 4), 4);
        assert_eq!(output.map(f32::to_bits), [0; 8]);
        assert_eq!(mr_pcm_ring_write(&raw mut ring, input.as_ptr(), 4), 4);
        mr_pcm_ring_restart(&raw mut ring);
        assert_eq!(mr_pcm_ring_write_silence(&raw mut ring, 0), 0);
        assert_eq!(mr_pcm_ring_read(&raw mut ring, output.as_mut_ptr(), 4), 0);
        assert_eq!(output.map(f32::to_bits), [0; 8]);
        assert_eq!(mr_pcm_ring_write_silence(&raw mut ring, 4), 4);
        assert_eq!(mr_pcm_ring_write_silence(&raw mut ring, 4), 4);
        assert_eq!(mr_pcm_ring_read(&raw mut ring, output.as_mut_ptr(), 4), 4);
        assert_eq!(output.map(f32::to_bits), [0; 8]);
    }
}

#[test]
fn byte_pcm_roundtrips_exact_samples_and_preserves_float_payloads() {
    let samples = [-1.0_f32, -0.5, 0.0, 0.5];
    for format in [
        SampleFormat::Float32,
        SampleFormat::Signed16,
        SampleFormat::Signed24,
        SampleFormat::Signed32,
    ] {
        let mut bytes = [0_u8; 16];
        let mut decoded = [0.0_f32; 4];
        let bytes = &mut bytes[..samples.len() * format.bytes_per_sample()];
        assert!(format.encode(&samples, bytes));
        assert!(format.decode(bytes, &mut decoded));
        assert_eq!(decoded.map(f32::to_bits), samples.map(f32::to_bits));
    }
    let payloads = [f32::from_bits(0x7fc1_2345), f32::from_bits(0x8000_0000)];
    let mut bytes = [0_u8; 8];
    let mut decoded = [0.0_f32; 2];
    assert!(SampleFormat::Float32.encode(&payloads, &mut bytes));
    assert!(SampleFormat::Float32.decode(&bytes, &mut decoded));
    assert_eq!(decoded.map(f32::to_bits), payloads.map(f32::to_bits));
}

#[test]
fn signed_pcm_clips_silences_nan_and_truncates_toward_zero() {
    let values = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 2.0, -2.0];
    for (format, maximum, minimum) in [
        (SampleFormat::Signed16, 32767_i32, -32768_i32),
        (SampleFormat::Signed24, 8_388_607, -8_388_608),
        (SampleFormat::Signed32, i32::MAX, i32::MIN),
    ] {
        let mut bytes = [0_u8; 20];
        let width = format.bytes_per_sample();
        assert!(format.encode(&values, &mut bytes[..5 * width]));
        for (actual, expected) in bytes[..5 * width]
            .chunks_exact(width)
            .zip([0_i32, maximum, minimum, maximum, minimum])
        {
            assert_eq!(actual, &expected.to_le_bytes()[..width]);
        }
    }
    let mut bytes = [0_u8; 4];
    assert!(SampleFormat::Signed16.encode(&[1.75 / 32768.0, -1.75 / 32768.0], &mut bytes));
    assert_eq!(bytes, [1, 0, 255, 255]);
}

#[test]
fn packed24_sign_extension_and_invalid_lengths_are_exact() {
    let bytes = [0, 0, 128, 255, 255, 127, 1, 0, 0, 255, 255, 255];
    let mut samples = [0.0_f32; 4];
    assert!(SampleFormat::Signed24.decode(&bytes, &mut samples));
    assert_eq!(
        samples.map(f32::to_bits),
        [
            -1.0_f32,
            1.0 - 1.0 / 8_388_608.0,
            1.0 / 8_388_608.0,
            -1.0 / 8_388_608.0
        ]
        .map(f32::to_bits)
    );
    let mut output = [17_u8; 13];
    assert!(!SampleFormat::Signed24.encode(&samples, &mut output));
    assert_eq!(output, [17; 13]);
    samples.fill(9.0);
    assert!(!SampleFormat::Signed24.decode(&bytes[..11], &mut samples));
    assert_eq!(samples.map(f32::to_bits), [9.0_f32.to_bits(); 4]);
}

#[test]
fn byte_ring_transfers_wrap_without_allocations_and_preserve_prefill() {
    for format in [
        SampleFormat::Float32,
        SampleFormat::Signed16,
        SampleFormat::Signed24,
        SampleFormat::Signed32,
    ] {
        let mut ring = PcmRing::new(2, 7).unwrap();
        let mut input = [0_u8; 16];
        let mut output = [9_u8; 16];
        let count = 4 * format.bytes_per_sample();
        assert!(format.encode(&[-1.0, -0.5, 0.0, 0.5], &mut input[..count]));
        let measured = allocation_counter::measure(|| {
            // SAFETY: one sequential producer/consumer, exact complete byte buffers,
            // disjoint from the retained ring mapping. Each block is two stereo frames.
            unsafe {
                assert_eq!(
                    mr_pcm_ring_read_pcm(
                        &raw mut ring,
                        output.as_mut_ptr().cast(),
                        2,
                        format as u32
                    ),
                    0
                );
                assert!(output[..count].iter().all(|&byte| byte == 0));
                assert_eq!(
                    mr_pcm_ring_write_pcm(&raw mut ring, input.as_ptr().cast(), 2, format as u32),
                    2
                );
                assert_eq!(
                    mr_pcm_ring_read_pcm(
                        &raw mut ring,
                        output.as_mut_ptr().cast(),
                        2,
                        format as u32
                    ),
                    0
                );
                assert_eq!(
                    mr_pcm_ring_write_pcm(&raw mut ring, input.as_ptr().cast(), 2, format as u32),
                    2
                );
                for _ in 0..100 {
                    assert_eq!(
                        mr_pcm_ring_read_pcm(
                            &raw mut ring,
                            output.as_mut_ptr().cast(),
                            2,
                            format as u32
                        ),
                        2
                    );
                    assert_eq!(output[..count], input[..count]);
                    assert_eq!(
                        mr_pcm_ring_write_pcm(
                            &raw mut ring,
                            input.as_ptr().cast(),
                            2,
                            format as u32
                        ),
                        2
                    );
                }
            }
        });
        assert_eq!(measured.count_total, 0);
        assert_eq!(ring.stats().overflows, 0);
    }
}

#[test]
fn byte_ring_unaligned_float_payloads_and_whole_block_overflow_are_preserved() {
    let mut ring = PcmRing::new(1, 4).unwrap();
    // Use the offset that is certainly unaligned, independent of stack placement.
    let mut input = [0_u8; 12];
    let input_offset = usize::from(input.as_ptr().addr().is_multiple_of(4));
    let payloads = [f32::from_bits(0x7fc1_2345), -0.0];
    assert!(SampleFormat::Float32.encode(&payloads, &mut input[input_offset..input_offset + 8]));
    let mut output = [17_u8; 12];
    let output_offset = usize::from(output.as_ptr().addr().is_multiple_of(4));
    // SAFETY: sequential SPSC test owner, complete disjoint byte buffers; the
    // chosen offsets deliberately exercise byte spans without float alignment.
    unsafe {
        for _ in 0..2 {
            assert_eq!(
                mr_pcm_ring_write_pcm(&raw mut ring, input.as_ptr().add(input_offset).cast(), 2, 1),
                2
            );
        }
        let overflowing = [0_u8; 4];
        assert_eq!(
            mr_pcm_ring_write_pcm(&raw mut ring, overflowing.as_ptr().cast(), 2, 2),
            0
        );
        assert_eq!(
            mr_pcm_ring_read_pcm(&raw mut ring, output.as_mut_ptr().cast(), 2, 99),
            0
        );
        assert_eq!(output, [17; 12]);
        assert_eq!(
            mr_pcm_ring_read_pcm(
                &raw mut ring,
                output.as_mut_ptr().add(output_offset).cast(),
                2,
                1
            ),
            2
        );
    }
    assert_eq!(
        output[output_offset..output_offset + 8],
        input[input_offset..input_offset + 8]
    );
    assert_eq!(ring.stats().overflows, 1);
    assert_eq!(ring.stats().queued, 2);
}

#[test]
fn byte_ring_zero_write_ack_and_invalid_format_do_not_publish_pcm() {
    for format in [
        SampleFormat::Signed16,
        SampleFormat::Signed24,
        SampleFormat::Signed32,
    ] {
        let mut ring = PcmRing::new(1, 4).unwrap();
        let input = [0_u8; 8];
        let mut output = [9_u8; 8];
        let count = 2 * format.bytes_per_sample();
        // SAFETY: sequential SPSC owner with two integer samples in complete
        // byte buffers. Null zero-frame input must ACK without reading input.
        unsafe {
            for _ in 0..2 {
                assert_eq!(
                    mr_pcm_ring_write_pcm(&raw mut ring, input.as_ptr().cast(), 2, format as u32),
                    2
                );
            }
            assert_eq!(
                mr_pcm_ring_write_pcm(&raw mut ring, input.as_ptr().cast(), 2, 99),
                0
            );
            mr_pcm_ring_restart(&raw mut ring);
            let epoch = ring.publication.requested_epoch.load(Relaxed);
            let written = ring.stats().written;
            assert_eq!(
                mr_pcm_ring_write_pcm(&raw mut ring, ptr::null(), 0, format as u32),
                0
            );
            assert_eq!(ring.publication.published_epoch.load(Relaxed), epoch);
            assert_eq!(ring.stats().written, written);
            assert_eq!(
                mr_pcm_ring_read_pcm(&raw mut ring, output.as_mut_ptr().cast(), 2, format as u32),
                0
            );
        }
        assert!(output[..count].iter().all(|&byte| byte == 0));
        assert!(output[count..].iter().all(|&byte| byte == 9));
        assert_eq!(ring.stats().read, ring.stats().written);
        assert_eq!(ring.stats().overflows, 0);
    }
}

#[test]
fn empty_and_failed_initialization_are_clearable_without_allocations() {
    let mut storage = std::mem::MaybeUninit::<PcmRing>::zeroed();
    // SAFETY: C-style zero initialization is the documented empty native owner.
    unsafe {
        mr_pcm_ring_clear(storage.as_mut_ptr());
    }
    for (channels, capacity) in [(0, 4), (65, 4), (1, 3), (1, (1 << 20) + 1)] {
        // SAFETY: sized/aligned exclusive cleared storage, no live callbacks.
        unsafe {
            assert!(!mr_pcm_ring_init(storage.as_mut_ptr(), channels, capacity));
            mr_pcm_ring_clear(storage.as_mut_ptr());
        }
    }
    assert!(storage::Storage::new(usize::MAX).is_err());
    // SAFETY: null calls do not access storage.
    unsafe {
        mr_pcm_ring_clear(ptr::null_mut());
        assert!(!mr_pcm_ring_init(ptr::null_mut(), 1, 4));
    }
}

#[test]
fn pcm_bits_wrap_for_non_power_of_two_capacity_and_every_channel_width() {
    for channels in 1..=64 {
        let mut ring = PcmRing::new(channels, 7).unwrap();
        let input: Vec<_> = (0..channels * 3)
            .map(|n| f32::from_bits(0x7fc0_0000 + n))
            .collect();
        let mut output = vec![0.0_f32; input.len()];
        for _ in 0..20 {
            // SAFETY: exclusive test owner, complete disjoint input/output spans.
            unsafe {
                assert_eq!(mr_pcm_ring_write(&raw mut ring, input.as_ptr(), 3), 3);
                assert_eq!(
                    mr_pcm_ring_read_available(&raw mut ring, output.as_mut_ptr(), 3),
                    3
                );
            }
            assert!(
                input
                    .iter()
                    .zip(&output)
                    .all(|(a, b)| a.to_bits() == b.to_bits())
            );
        }
    }
}

#[test]
fn zero_frame_write_acknowledges_a_full_stale_epoch_before_capacity_check() {
    let mut ring = PcmRing::new(1, 4).unwrap();
    let input = [1.0_f32; 2];
    let mut output = [9.0_f32; 2];
    // SAFETY: one sequential producer/consumer with complete buffers.
    unsafe {
        for _ in 0..2 {
            assert_eq!(mr_pcm_ring_write(&raw mut ring, input.as_ptr(), 2), 2);
        }
        mr_pcm_ring_restart(&raw mut ring);
        assert_eq!(mr_pcm_ring_write(&raw mut ring, ptr::null(), 0), 0);
        assert_eq!(
            mr_pcm_ring_read_available(&raw mut ring, output.as_mut_ptr(), 2),
            0
        );
    }
    assert_eq!(ring.stats().read, ring.stats().written);
    assert_eq!(output.map(f32::to_bits), [0; 2]);
    assert_eq!(ring.stats().overflows, 0);
}

#[test]
fn partial_epoch_publication_never_advances_the_consumer_or_spins() {
    let mut ring = PcmRing::new(1, 8).unwrap();
    ring.publication.written.store(4, Relaxed);
    ring.publication.epoch_sequence.store(1, SeqCst);
    ring.publication.epoch_start.store(4, SeqCst);
    let mut output = [9.0_f32; 2];
    // SAFETY: single consumer, initialized owner; writer publication intentionally stalled.
    assert_eq!(
        unsafe { mr_pcm_ring_read_available(&raw mut ring, output.as_mut_ptr(), 2) },
        0
    );
    assert_eq!(output.map(f32::to_bits), [0; 2]);
    assert_eq!(ring.stats().read, 0);
}

#[test]
fn restart_during_a_read_invalidates_the_read_commit_and_does_not_rewind_discard() {
    let ring = PcmRing::new(1, 8).unwrap();
    ring.publication.written.store(4, Relaxed);
    let mut reader = Reader::default();
    let window = ring.reserve_read(&mut reader, 2, false).unwrap();
    ring.publication.requested_epoch.store(1, Relaxed);
    assert!(!ring.finish_read(&mut reader, &window, 2));
    assert!(!reader.primed);
    ring.publication.read.store(4, Relaxed);
    ring.publication.epoch_start.store(2, SeqCst);
    ring.publication.published_epoch.store(1, SeqCst);
    assert!(ring.reserve_read(&mut reader, 2, false).is_none());
    assert_eq!(ring.stats().read, 4);
}

#[test]
fn publication_changes_during_observation_are_rejected_without_cursor_movement() {
    let ring = PcmRing::new(1, 8).unwrap();
    ring.publication.written.store(4, Relaxed);
    let mut writer = 0;
    assert!(
        ring.observed_epoch(|| {
            ring.publication.requested_epoch.store(1, Relaxed);
            assert!(ring.reserve_write(&mut writer, 0).is_none());
        })
        .is_none()
    );
    assert_eq!(ring.stats().read, 0);
    assert_eq!(ring.observed_epoch(|| {}), Some((1, 4)));
}

#[test]
fn actual_read_path_silences_pcm_if_restart_arrives_after_the_copy() {
    let mut ring = PcmRing::new(1, 8).unwrap();
    let input = [1.0_f32; 2];
    let mut output = [9.0_f32; 2];
    // SAFETY: exclusive owner with sized input/output; test requests restart using
    // the same atomic control boundary while the consumer owns its reader cell.
    unsafe {
        assert_eq!(mr_pcm_ring_write(&raw mut ring, input.as_ptr(), 2), 2);
        assert_eq!(
            ffi::read(&raw mut ring, output.as_mut_ptr(), 2, false, |ring| {
                ring.publication
                    .requested_epoch
                    .fetch_add(1, Ordering::Release);
            }),
            0
        );
    }
    assert_eq!(output.map(f32::to_bits), [0; 2]);
    assert_eq!(ring.stats().read, 2);
}

#[test]
fn mapping_failure_returns_an_error_without_publishing_storage() {
    // SAFETY: injected MAP_FAILED owns no memory and must never reach unmap.
    let failure = unsafe {
        storage::Storage::map_with(8, |bytes| {
            assert_eq!(bytes, 32);
            ptr::without_provenance_mut(usize::MAX)
        })
    };
    assert!(failure.is_err());
}

#[test]
fn control_prefault_preserves_sample_bits_without_allocations() {
    let mut samples: Vec<f32> = (0..8193)
        .map(|index| f32::from_bits(0x7fc0_0000 + index))
        .collect();
    let expected: Vec<u32> = samples.iter().map(|sample| sample.to_bits()).collect();
    let measured = allocation_counter::measure(|| {
        prefault_pcm(&mut samples[1..]); // Unaligned start and a partial final page.
        prefault_pcm(&mut []);
    });
    assert_eq!(measured.count_total, 0);
    assert!(
        samples
            .iter()
            .zip(expected)
            .all(|(sample, bits)| sample.to_bits() == bits)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn all_pcm_pages_are_resident_before_callback_publication() {
    unsafe extern "C" {
        fn getpagesize() -> std::ffi::c_int;
        fn mincore(address: *mut std::ffi::c_void, length: usize, vector: *mut u8) -> i32;
    }
    let ring = PcmRing::new(64, 32768).unwrap();
    let bytes = 64 * 32768 * size_of::<f32>();
    // SAFETY: getpagesize has no arguments or borrowed state.
    let page_bytes = usize::try_from(unsafe { getpagesize() }).unwrap();
    let mut residency = vec![0; bytes.div_ceil(page_bytes)];
    // SAFETY: the ring owns a page-aligned live mapping of bytes, and residency
    // has one writable byte per page. Query before reading or writing any PCM.
    assert_eq!(
        unsafe { mincore(ring.storage.samples.cast(), bytes, residency.as_mut_ptr()) },
        0
    );
    assert!(residency.iter().all(|state| state & 1 != 0));
    // Preparation must not publish PCM or alter the initial FIFO cursors.
    assert_eq!(ring.stats().written, 0);
    assert_eq!(ring.stats().read, 0);
    assert_eq!(ring.stats().queued, 0);
}

#[test]
fn retirement_returns_mapped_pages_to_the_os_in_an_isolated_process() {
    unsafe extern "C" {
        fn mprotect(address: *mut std::ffi::c_void, length: usize, protection: i32) -> i32;
    }
    const TEST: &str = "tests::retirement_returns_mapped_pages_to_the_os_in_an_isolated_process";
    const MARKER: &str = "MIXRELAY_PCM_RETIREMENT_CHILD";
    if std::env::var_os(MARKER).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--test-threads=1"])
            .env(MARKER, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    for _ in 0..64 {
        let ring = PcmRing::new(64, 65536).unwrap();
        let address = ring.storage.samples.cast();
        // SAFETY: the owned mapping is page-aligned; retain its existing read/write
        // protection. Unlike mincore on Darwin, mprotect rejects unmapped pages.
        assert_eq!(unsafe { mprotect(address, 1, 1 | 2) }, 0);
        drop(ring);
        // SAFETY: checking this now-unmapped page does not dereference it; the
        // isolated child performs no allocation between drop and this check.
        assert_eq!(unsafe { mprotect(address, 1, 1 | 2) }, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(12));
    }
}

#[test]
fn callbacks_allocate_nothing_and_repeated_retirement_clears_the_owner() {
    for _ in 0..64 {
        let mut ring = PcmRing::new(64, 65536).unwrap();
        let input = [0.5_f32; 64 * 32];
        let mut output = [0.0_f32; 64 * 32];
        let count = allocation_counter::measure(|| {
            for _ in 0..100 {
                // SAFETY: exclusive native test owner and fully sized caller buffers.
                unsafe {
                    assert_eq!(mr_pcm_ring_write(&raw mut ring, input.as_ptr(), 32), 32);
                    assert_eq!(
                        mr_pcm_ring_read_available(&raw mut ring, output.as_mut_ptr(), 32),
                        32
                    );
                }
            }
        });
        assert_eq!(count.count_total, 0);
        assert_eq!(count.count_current, 0);
        let address = ring.storage.samples;
        // SAFETY: producer/consumer are stopped; native clear must release mapping and be idempotent.
        unsafe {
            mr_pcm_ring_clear(&raw mut ring);
            mr_pcm_ring_clear(&raw mut ring);
        }
        assert!(!address.is_null());
        assert!(ring.storage.samples.is_null());
    }
}
