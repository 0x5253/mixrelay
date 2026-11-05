//! Graph reads: format handshake, PCM transfer, faults and rebuffering.
use super::*;

#[test]
fn two_independent_shared_sources_read_the_same_event_at_the_same_acquisition_time() {
    use crate::application_source_transport::mr_application_source_transport_write_timed;
    let registry = registry();
    let mut owners = [
        producer(&registry, &request("timed-a")),
        producer(&registry, &request("timed-b")),
    ];
    let tokens = [token(b"timed-a"), token(b"timed-b")];
    let mut output = [[0.0; 256]; 2];
    for index in 0..2 {
        // Request format using the actual consumer handshake, then reset its real producer.
        registry.read_audio(tokens[index], &mut output[index], 128, 48_000);
        let source_read = registry.read(tokens[index]);
        let source = source_read.source().unwrap();
        let format = source.mapping.clock().format_word.load(Ordering::Acquire);
        assert!(unsafe {
            mr_application_source_transport_reset(&raw mut owners[index], format, 48_000)
        });
        let offset = index * 128;
        let mut pcm = [0.0; 1536];
        let mut times = [0_u64; 768];
        for (frame, time) in times.iter_mut().enumerate() {
            *time = 1_000_000_000 + (frame + offset) as u64 * 1_000_000_000 / 48_000;
            if frame + offset == 512 {
                pcm[frame * 2] = 1.0;
                pcm[frame * 2 + 1] = -1.0;
            }
        }
        assert!(unsafe {
            mr_application_source_transport_write_timed(
                &raw mut owners[index],
                pcm.as_ptr(),
                times.as_ptr(),
                768,
            )
        });
        unsafe {
            mr_application_source_transport_publish_native(&raw const owners[index], 768, true);
        };
    }
    let target = 1_000_000_000 + 512 * 1_000_000_000_u64 / 48_000;
    let measured = allocation_counter::measure(|| {
        for index in 0..2 {
            assert_eq!(
                registry.read_at(tokens[index], &mut output[index], 48_000, target),
                128
            );
        }
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(output[0].map(f32::to_bits), output[1].map(f32::to_bits));
    assert_eq!(&output[0][..2], &[1.0, -1.0]);
    assert_eq!(
        registry.read_at(tokens[0], &mut output[0], 48_000, target),
        0
    );
    assert!(
        output[0].iter().all(|sample| sample.to_bits() == 0),
        "a common cursor must not replay a consumed range"
    );
    assert_eq!(
        registry.read_at(tokens[0], &mut output[0], 48_000, target + 1_000_000_000),
        0
    );
    assert!(output[0].iter().all(|sample| sample.to_bits() == 0));
}

#[test]
fn normalized_samples_report_once_even_when_shared_source_pcm_is_not_available() {
    use crate::application_source_transport::{
        SourceClockSnapshot, mr_application_source_transport_clock,
        mr_application_source_transport_record_source_faults,
    };
    let registry = registry();
    let request = request("normalized");
    let mut owner = producer(&registry, &request);
    let token = token(b"normalized");
    let source_read = registry.read(token);
    let source = source_read.source().unwrap();
    let format =
        unsafe { mr_application_clock_request_rate(source.mapping.clock.as_ptr(), 48_000) };
    assert!(unsafe { mr_application_source_transport_reset(&raw mut owner, format, 48_000) });
    let mut clock = SourceClockSnapshot::default();
    assert!(unsafe { mr_application_source_transport_clock(&raw const owner, &raw mut clock) });
    assert!(unsafe {
        mr_application_source_transport_record_source_faults(&raw mut owner, clock.epoch, 3)
    });
    let mut output = [9.0; 256];
    let measured = allocation_counter::measure(|| {
        assert_eq!(registry.read_audio(token, &mut output, 128, 48_000), 0);
        assert_eq!(registry.take_source_faults(token), 3);
        assert_eq!(registry.take_source_faults(token), 0);
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(output.map(f32::to_bits), [0.0_f32.to_bits(); 256]);
    assert!(unsafe {
        mr_application_source_transport_record_source_faults(&raw mut owner, clock.epoch, 2)
    });
    assert!(unsafe {
        mr_application_source_transport_write(&raw mut owner, [0.25; 1536].as_ptr(), 768)
    });
    unsafe { mr_application_source_transport_publish_native(&raw const owner, 768, true) };
    assert_eq!(registry.read_audio(token, &mut output, 128, 48_000), 128);
    assert_eq!(output.map(f32::to_bits), [0.25_f32.to_bits(); 256]);
    assert_eq!(registry.take_source_faults(token), 2);
    assert_eq!(registry.take_source_faults(token), 0);
    assert!(unsafe {
        mr_application_source_transport_record_source_faults(&raw mut owner, clock.epoch, 1)
    });
    drop(owner);
    assert_eq!(registry.take_source_faults(token), 1);
    assert_eq!(registry.take_source_faults(token), 0);
}

#[test]
fn consumer_waits_for_current_format_ack_then_transfers_actual_pcm() {
    let registry = registry();
    let attachment = registry.attach(&request("one")).unwrap();
    let read = registry.read(token(b"one"));
    let source = read.source().unwrap();
    let mut output = [9.0; 256];
    assert_eq!(
        registry.read_audio(source.token, &mut output, 128, 44_100),
        0
    );
    let clock = source.mapping.clock();
    clock
        .acknowledged_format
        .store(clock.format_word.load(Ordering::Acquire), Ordering::Release);
    clock.native_sample_rate.store(48_000, Ordering::Release);
    clock.native_running.store(1, Ordering::Release);
    assert!(unsafe {
        mr_shared_audio_buffer_write(
            source.mapping.buffer.as_ptr(),
            attachment.source_epoch,
            0,
            [0.25; 1536].as_ptr(),
            768,
        )
    });
    assert_eq!(
        registry.read_audio(source.token, &mut output, 128, 44_100),
        128
    );
    assert!(
        output
            .iter()
            .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
    );
    let snapshot = registry.clock(source.token).unwrap();
    assert_eq!(
        snapshot.epoch,
        (attachment.producer_lease << 16) | attachment.source_epoch
    );
    assert_eq!(snapshot.graph_rate, 44_100);
    assert_eq!(snapshot.native_rate, 48_000);
    assert_eq!(
        registry.read_audio(source.token, &mut output, 128, 96_000),
        0
    );
    assert!(registry.clock(source.token).is_none());
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "bit-exact numbered PCM proves no replay or splice"
)]
fn adaptive_rebuffer_holds_numbered_pcm_cursor_without_replay_or_allocation() {
    let registry = registry();
    let request = SourceRequest {
        capacity_frames: 2048,
        channels: 1,
        ..request("adaptive")
    };
    let mut owner = producer(&registry, &request);
    let read = registry.read(token(b"adaptive"));
    let source = read.source().unwrap();
    let clock = source.mapping.clock();
    let format =
        unsafe { mr_application_clock_request_rate(source.mapping.clock.as_ptr(), 48_000) };
    assert!(unsafe { mr_application_source_transport_reset(&raw mut owner, format, 48_000) });
    unsafe {
        mr_application_source_transport_publish_native(&raw const owner, 512, true);
        crate::application_source_transport::mr_application_source_transport_publish_measurements(
            &raw const owner,
            128,
            128,
            0,
        );
    }
    let numbered: Vec<f32> = (0_u16..4096).map(f32::from).collect();
    let write = |owner: &mut SourceTransport, start: usize, frames: u32| {
        assert!(unsafe {
            mr_application_source_transport_write(owner, numbered[start..].as_ptr(), frames)
        });
    };
    let mut output = [0.0; 128];
    let consume = |output: &mut [f32]| {
        // Explicit separated scheduling bursts, without sleeping in a test.
        unsafe {
            (*source.timeline.get()).separate_next_read();
        }
        registry.read_audio(source.token, output, 128, 48_000)
    };
    write(&mut owner, 0, 512);
    let measured = allocation_counter::measure(|| {
        assert_eq!(consume(&mut output), 128);
        assert_eq!(output, numbered[256..384]);
        assert_eq!(consume(&mut output), 128);
        assert_eq!(output, numbered[384..512]);
        assert_eq!(consume(&mut output), 0);
        assert_eq!(clock.target_backlog_frames.load(Ordering::Acquire), 384);
        assert_eq!(
            clock.consumer_position.load(Ordering::Acquire) & crate::application_clock::FRAME_MASK,
            512
        );
        write(&mut owner, 512, 128);
        for _ in 0..4 {
            assert_eq!(consume(&mut output), 0);
            assert_eq!(output, [0.0; 128]);
        }
        assert_eq!(clock.starvation_episodes.load(Ordering::Acquire), 1);
        assert_eq!(clock.starvation_frames.load(Ordering::Acquire), 128);
        assert_eq!(clock.rebuffer_frames.load(Ordering::Acquire), 640);
        assert_eq!(clock.consumer_running.load(Ordering::Acquire), 0);
        write(&mut owner, 640, 256);
        assert_eq!(consume(&mut output), 128);
        assert_eq!(output, numbered[512..640]);
        assert_eq!(consume(&mut output), 128);
        assert_eq!(output, numbered[640..768]);
        assert_eq!(clock.overwrite_skips.load(Ordering::Acquire), 0);
    });
    assert_eq!(measured.count_total, 0);
    write(&mut owner, 896, 2048);
    assert_eq!(consume(&mut output), 128);
    assert_eq!(output, numbered[2560..2688]);
    assert_eq!(clock.overwrite_skips.load(Ordering::Acquire), 1792);
}
