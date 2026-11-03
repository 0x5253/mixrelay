use super::*;
use crate::endpoint_transport::EndpointTransportConfig;

fn writer(client: u32) -> ClientPcmWriter {
    let mut writer = ClientPcmWriter::new(
        EndpointTransportConfig {
            endpoint_id: [1; 16],
            capacity_frames: 16_384,
            render_channels: 2,
            capture_channels: 2,
        },
        client,
    )
    .unwrap();
    assert!(writer.set_clock((2, 10_000), 48_000, true, true));
    writer
}

fn reader(writer: &ClientPcmWriter) -> ClientPcmReader {
    let mut snapshot = ClientPcmSnapshot::new().unwrap();
    assert!(snapshot.push(writer));
    // SAFETY: snapshot retains the complete, producer-created VM grant for this decode.
    unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }
        .unwrap()
        .remove(0)
}

#[test]
fn retained_remote_mapping_reads_silence_and_signal_at_absolute_positions() {
    let writer = writer(42);
    let reader = reader(&writer);
    assert!(writer.write(200_000, &[0.375; 256], 128));
    assert!(writer.write(200_128, &[0.0; 256], 128));
    assert_eq!(reader.clock().unwrap().first, Some(200_000));
    let measured = allocation_counter::measure(|| {
        let clock = reader.clock().unwrap();
        let mut output = [7.0; 512];
        let mut coverage = [false; 256];
        assert!(reader.read(clock, 200_000, &mut output, &mut coverage));
        assert_eq!(output[..256], [0.375; 256]);
        assert_eq!(output[256..], [0.0; 256]);
        assert!(coverage.into_iter().all(|valid| valid));
    });
    assert_eq!(measured.count_total, 0);
}

#[test]
fn source_coordinates_survive_a_consumers_small_read_gap_without_replaying_the_old_period() {
    let writer = writer(42);
    let reader = reader(&writer);
    // The old CoreAudio cache failure reused PCM exactly 16,384 frames earlier.
    let first = 400_000;
    assert!(writer.write(first - 16_384, &[0.75; 1024], 512));
    assert!(writer.write(first, &[0.0; 1024], 512));
    let clock = reader.clock().unwrap();
    let mut short = [1.0; 256];
    let mut short_coverage = [false; 128];
    assert!(reader.read(clock, first, &mut short, &mut short_coverage));
    // A separate reader asks for the entire interval after a partial consumer skipped it.
    let mut output = [1.0; 1024];
    let mut coverage = [false; 512];
    assert!(reader.read(clock, first, &mut output, &mut coverage));
    assert_eq!(output.map(f32::to_bits), [0.0_f32.to_bits(); 1024]);
    assert!(coverage.into_iter().all(|valid| valid));
}

#[test]
fn producer_removal_invalidates_a_retained_reader_before_its_vm_is_reclaimed() {
    let writer = writer(7);
    let reader = reader(&writer);
    assert!(writer.write(100, &[0.5; 16], 8));
    let clock = reader.clock().unwrap();
    drop(writer);
    assert!(reader.clock().is_none());
    let mut output = [9.0; 16];
    let mut coverage = [true; 8];
    assert!(!reader.read(clock, 100, &mut output, &mut coverage));
    assert_eq!(output.map(f32::to_bits), [0.0_f32.to_bits(); 16]);
    assert_eq!(coverage, [false; 8]);
}

#[test]
fn restart_rejects_the_previous_clock_and_never_compacts_unpublished_frames() {
    let mut writer = writer(7);
    let reader = reader(&writer);
    assert!(writer.write(100, &[0.5; 16], 8));
    let old = reader.clock().unwrap();
    assert!(writer.set_clock((3, 20_000), 96_000, true, true));
    assert!(writer.write(100, &[0.25; 8], 4));
    assert!(writer.write(106, &[0.375; 4], 2));
    let mut output = [9.0; 16];
    let mut coverage = [true; 8];
    assert!(!reader.read(old, 100, &mut output, &mut coverage));
    assert_eq!(output.map(f32::to_bits), [0.0_f32.to_bits(); 16]);
    assert!(reader.read(reader.clock().unwrap(), 100, &mut output, &mut coverage));
    assert_eq!(
        output.map(f32::to_bits),
        [
            0.25_f32, 0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 0.0, 0.0, 0.0, 0.0, 0.375, 0.375,
            0.375, 0.375
        ]
        .map(f32::to_bits)
    );
    assert_eq!(coverage, [true, true, true, true, false, false, true, true]);
}

#[test]
fn malformed_or_duplicate_snapshots_do_not_publish_a_partial_authoritative_list() {
    let writer = writer(7);
    let mut snapshot = ClientPcmSnapshot::new().unwrap();
    assert!(snapshot.push(&writer));
    assert!(snapshot.push(&writer));
    // SAFETY: both pointers are either null or owned live XPC dictionaries.
    unsafe {
        assert!(matches!(
            ClientPcmSnapshot::decode(std::ptr::null_mut()),
            Err(SnapshotError::Invalid)
        ));
        assert!(matches!(
            ClientPcmSnapshot::decode(snapshot.as_raw()),
            Err(SnapshotError::Invalid)
        ));
    }
}

#[test]
fn endpoint_clock_transition_excludes_every_client_until_the_complete_set_is_ready() {
    use std::sync::Arc;
    let owner = Arc::new(ClientPcmClockOwner::new().unwrap());
    let config = EndpointTransportConfig {
        endpoint_id: [1; 16],
        capacity_frames: 16_384,
        render_channels: 2,
        capture_channels: 2,
    };
    let mut first = ClientPcmWriter::with_clock(config, 7, owner.clone()).unwrap();
    let mut second = ClientPcmWriter::with_clock(config, 8, owner.clone()).unwrap();
    let old = ClientPcmTimeline {
        epoch: 2,
        anchor: 1_000,
        rate: 48_000,
        running: true,
    };
    let publication = owner.begin(old).unwrap();
    assert!(publication.configure(&mut first, true));
    assert!(publication.configure(&mut second, true));
    publication.commit();
    let readers = [reader(&first), reader(&second)];
    assert!(first.write(100, &[0.5; 16], 8));
    assert!(second.write(100, &[0.25; 16], 8));
    assert!(readers.iter().all(|reader| reader.clock().is_some()));
    let next = ClientPcmTimeline {
        epoch: 3,
        anchor: 2_000,
        rate: 96_000,
        running: true,
    };
    let publication = owner.begin(next).unwrap();
    assert!(publication.configure(&mut first, true));
    // The second queue still contains old PCM. No consumer may use it while
    // the endpoint is between generations, even though its own header is stable.
    assert!(readers.iter().all(|reader| reader.clock().is_none()));
    assert!(publication.configure(&mut second, true));
    publication.commit();
    assert!(readers.iter().all(|reader| {
        reader
            .clock()
            .is_some_and(|clock| clock.epoch == 3 && clock.first.is_none())
    }));
    owner.retire();
    assert!(readers.iter().all(|reader| reader.clock().is_none()));
}

#[test]
fn abandoned_clock_transition_stays_silent_and_replacement_cannot_reuse_its_identity() {
    use std::sync::Arc;
    let owner = Arc::new(ClientPcmClockOwner::new().unwrap());
    let config = EndpointTransportConfig {
        endpoint_id: [1; 16],
        capacity_frames: 16_384,
        render_channels: 2,
        capture_channels: 2,
    };
    let mut first = ClientPcmWriter::with_clock(config, 7, owner.clone()).unwrap();
    let timeline = ClientPcmTimeline {
        epoch: 2,
        anchor: 1_000,
        rate: 48_000,
        running: true,
    };
    let publication = owner.begin(timeline).unwrap();
    assert!(publication.configure(&mut first, true));
    publication.commit();
    let original = reader(&first);
    let old_identity = original.clock().unwrap().incarnation;
    let incomplete = owner
        .begin(ClientPcmTimeline {
            epoch: 3,
            ..timeline
        })
        .unwrap();
    assert!(incomplete.configure(&mut first, true));
    drop(incomplete);
    assert!(original.clock().is_none());
    let mut replacement = writer(7);
    assert!(replacement.set_clock((2, 1_000), 48_000, true, true));
    assert_ne!(
        reader(&replacement).clock().unwrap().incarnation,
        old_identity
    );
}

#[test]
fn membership_handoff_keeps_the_window_until_all_client_grants_arrive() {
    use std::sync::Arc;
    let owner = Arc::new(ClientPcmClockOwner::new().unwrap());
    let config = EndpointTransportConfig {
        endpoint_id: [1; 16],
        capacity_frames: 16_384,
        render_channels: 2,
        capture_channels: 2,
    };
    let mut first = ClientPcmWriter::with_clock(config, 7, owner.clone()).unwrap();
    let timeline = ClientPcmTimeline {
        epoch: 2,
        anchor: 1_000,
        rate: 48_000,
        running: true,
    };
    let publication = owner.begin(timeline).unwrap();
    assert!(publication.configure(&mut first, true));
    publication.commit();
    assert!(first.write(100, &[0.125; 256], 128));
    let old = vec![reader(&first)];
    let mut mixer = ClientPcmMixer::default();
    let window = mixer.window(&old, |_| Some(48_000)).unwrap();
    owner.publish_membership(2);
    let mut joined = ClientPcmWriter::with_clock(config, 8, owner.clone()).unwrap();
    let publication = owner.begin(timeline).unwrap();
    assert!(publication.configure(&mut joined, true));
    publication.commit();
    assert!(joined.write(100, &[0.25; 256], 128));
    let mut output = [9.0; 256];
    let mut scratch = [0.0; 256];
    assert_eq!(
        ClientPcmMixer::mix(&old, window, &mut output, &mut scratch),
        Err(MixError::Changed)
    );
    assert_eq!(output.map(f32::to_bits), [0.0_f32.to_bits(); 256]);
    assert!(mixer.window(&old, |_| Some(48_000)).is_none());
    let current = vec![reader(&first), reader(&joined)];
    let current_window = mixer.window(&current, |_| Some(48_000)).unwrap();
    assert_eq!(current_window.first, window.first);
    assert!(current_window.discontinuity);
    ClientPcmMixer::mix(&current, current_window, &mut output, &mut scratch).unwrap();
    assert_eq!(output.map(f32::to_bits), [0.375_f32.to_bits(); 256]);
    mixer.commit(current_window);
    assert!(mixer.window(&current, |_| Some(48_000)).is_none());
}

fn queue(channels: u32) -> ClientPcmWriter {
    let mut writer = ClientPcmWriter::new(
        EndpointTransportConfig {
            endpoint_id: [1; 16],
            capacity_frames: QUEUE_FRAMES,
            render_channels: channels,
            capture_channels: channels,
        },
        42,
    )
    .unwrap();
    assert!(writer.set_clock((2, 10_000), 48_000, true, true));
    writer
}

#[test]
fn a_prepared_queue_is_resident_before_publication_and_reports_every_frame_missing() {
    unsafe extern "C" {
        fn getpagesize() -> i32;
    }
    let writer = queue(64);
    let (_, bytes) = writer.region();
    // Preparation runs on control outside the IO freeze. Leaving pages to kernel zero-fill
    // moved one fault per page (over 1,000 at 64 channels) onto the HAL IO thread. The
    // count comes from the pre-fault itself: residency read back later could change under
    // memory pressure.
    // SAFETY: getpagesize has no preconditions.
    let page = usize::try_from(unsafe { getpagesize() }).unwrap();
    assert!(bytes.div_ceil(page) > 1_000);
    assert_eq!(writer.prefaulted(), bytes.div_ceil(page));
    let reader = reader(&writer);
    let clock = reader.clock().unwrap();
    let mut output = vec![9.0; 64 * 128];
    let mut coverage = [true; 128];
    for frame in [0, 1, u64::from(QUEUE_FRAMES) - 128, 1 << 40] {
        assert!(reader.read(clock, frame, &mut output, &mut coverage));
        assert!(coverage.iter().all(|valid| !valid));
        assert!(output.iter().all(|sample| sample.to_bits() == 0));
    }
}

#[test]
fn reused_slots_never_expose_wrapped_or_previous_epoch_pcm() {
    let mut writer = queue(2);
    let reader = reader(&writer);
    let capacity = u64::from(QUEUE_FRAMES);
    for block in 0..(capacity + 256) / 128 {
        let value = if block * 128 < capacity { 0.25 } else { 0.5 };
        assert!(writer.write(block * 128, &[value; 256], 128));
    }
    let clock = reader.clock().unwrap();
    let mut output = [9.0; 512];
    let mut coverage = [true; 256];
    // The first 256 frames' slots now hold frames one ring later.
    assert!(reader.read(clock, 0, &mut output, &mut coverage));
    assert!(coverage.iter().all(|valid| !valid));
    assert!(output.iter().all(|sample| sample.to_bits() == 0));
    assert!(reader.read(clock, capacity, &mut output, &mut coverage));
    assert!(coverage.iter().all(|valid| *valid));
    assert_eq!(output.map(f32::to_bits), [0.5_f32.to_bits(); 512]);
    // A rate rearm keeps the mapping but retires every committed slot.
    assert!(writer.set_clock((3, 20_000), 96_000, true, true));
    let rearmed = reader.clock().unwrap();
    assert_eq!(rearmed.rate, 96_000);
    assert!(reader.read(rearmed, capacity, &mut output, &mut coverage));
    assert!(coverage.iter().all(|valid| !valid));
    assert!(output.iter().all(|sample| sample.to_bits() == 0));
}

#[test]
fn the_queue_covers_a_full_render_span_lead_over_a_pump_170_ms_late_at_192_khz() {
    let span = u64::from(MAX_RENDER_SPAN_FRAMES);
    let target = 0;
    // Twice the span: 32,768 frames, 170 ms at 192 kHz, beyond ReadInput's own lag.
    let lag = 2 * span;
    let writer = queue(2);
    let readers = vec![reader(&writer)];
    let word = |frame: u64| f32::from(u16::try_from(frame % 8192).unwrap()) / 8192.0;
    let mut mixer = ClientPcmMixer::default();
    let (mut written, mut consumed) = (0, 0);
    let mut samples = vec![0.0; usize::try_from(span).unwrap() * 2];
    let mut output = [0.0; 256];
    let mut scratch = [0.0; 256];
    // The host renders whole coalesced spans one target ahead of the playhead, and the
    // pump wakes only after the playhead has moved twice that lead past its cursor.
    for playhead in (span..8 * u64::from(QUEUE_FRAMES)).step_by(usize::try_from(span).unwrap()) {
        while written < playhead + target + span {
            for (index, frame) in samples.chunks_exact_mut(2).enumerate() {
                frame.fill(word(written + index as u64));
            }
            assert!(writer.write(written, &samples, MAX_RENDER_SPAN_FRAMES));
            written += span;
        }
        if playhead - consumed < lag {
            continue;
        }
        while let Some(window) = mixer.window(&readers, |_| Some(playhead)) {
            assert_eq!(
                window.discontinuity,
                consumed == 0,
                "lost PCM at {consumed}"
            );
            assert_eq!(window.first, consumed);
            let frames = window.frames as usize;
            ClientPcmMixer::mix(&readers, window, &mut output[..frames * 2], &mut scratch).unwrap();
            for (index, frame) in output[..frames * 2].chunks_exact(2).enumerate() {
                assert_eq!(frame, [word(consumed + index as u64); 2]);
            }
            mixer.commit(window);
            consumed += u64::from(window.frames);
        }
        assert_eq!(consumed, playhead);
    }
}

#[test]
fn the_mixer_counts_published_frames_it_skips_after_an_overrun_or_a_late_join() {
    let writer = queue(2);
    let readers = vec![reader(&writer)];
    let capacity = u64::from(QUEUE_FRAMES);
    let mut written = 0;
    let mut write_through = |end: u64| {
        while written < end {
            assert!(writer.write(written, &[0.25; 256], 128));
            written += 128;
        }
    };
    let mut output = [0.0; 256];
    let mut scratch = [0.0; 256];
    write_through(512);
    let mut mixer = ClientPcmMixer::default();
    let window = mixer.window(&readers, |_| Some(512)).unwrap();
    assert_eq!((window.first, window.discontinuity), (0, true));
    ClientPcmMixer::mix(&readers, window, &mut output, &mut scratch).unwrap();
    mixer.commit(window);
    assert_eq!(mixer.skipped_frames(), 0);
    // The cursor stalls at 128 while the writer overruns it by 1,024 frames.
    let end = 128 + capacity + 1_024;
    write_through(end);
    let window = mixer.window(&readers, |_| Some(end)).unwrap();
    assert_eq!((window.first, window.discontinuity), (end - capacity, true));
    assert_eq!(mixer.skipped_frames(), 1_024);
    // A reader that first observes this timeline now has never seen its first frames.
    let mut late = ClientPcmMixer::default();
    let window = late.window(&readers, |_| Some(end)).unwrap();
    assert_eq!(window.first, end - capacity);
    assert_eq!(late.skipped_frames(), end - capacity);
}
