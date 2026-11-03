//! Real shared consumer publication cannot turn a committed producer reset into failure.
use super::*;
use mixrelay_macos_transport::{
    application_clock::{ApplicationClock, mr_application_clock_attach},
    application_source_transport::{SourceClockRead, mr_application_source_transport_attach},
    source_control::SourceAttachment,
};
use std::{
    ffi::c_void,
    os::fd::{AsRawFd, IntoRawFd},
};

struct ConsumerMapping {
    address: *mut c_void,
    bytes: usize,
    clock: *mut ApplicationClock,
}
impl Drop for ConsumerMapping {
    fn drop(&mut self) {
        // SAFETY: the test finished all atomic accesses; this owns its independent mmap.
        unsafe { libc::munmap(self.address, self.bytes) };
    }
}

fn source() -> (SourceRegistry, SourceTransport, ConsumerMapping) {
    let registry = crate::endpoint_service::virtual_pcm::test_storage::registry(Box::new(Clock {
        nanos: Arc::new(AtomicU64::new(0)),
    }));
    let mut request = SourceRequest {
        capacity_frames: 16_384,
        channels: 2,
        transport_version: VERSION,
        ..SourceRequest::default()
    };
    request.source_id[0] = 97;
    let grant = registry.attach(&request).unwrap();
    // SAFETY: the registry owns a complete immutable mapping; this test retains the mmap.
    let address = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            grant.mapped_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            grant.descriptor.as_raw_fd(),
            0,
        )
    };
    assert_ne!(address, libc::MAP_FAILED);
    // SAFETY: the grant specifies the retained backing extent and exact geometry.
    let clock = unsafe { mr_application_clock_attach(address, grant.mapped_size, 16_384, 2) };
    assert!(!clock.is_null());
    let mapping = ConsumerMapping {
        address,
        bytes: grant.mapped_size,
        clock,
    };
    let mut attachment = SourceAttachment {
        descriptor: grant.descriptor.into_raw_fd(),
        mapped_size: grant.mapped_size,
        producer_lease: grant.producer_lease,
        source_epoch: grant.source_epoch,
    };
    // SAFETY: transfer this grant's unique descriptor through the real producer facade.
    let owner = unsafe {
        mr_application_source_transport_attach(&raw mut attachment, 16_384, 2, std::ptr::null_mut())
    };
    assert!(!owner.is_null());
    // SAFETY: recover the uniquely owned allocation created by that facade.
    let transport = *unsafe { Box::from_raw(owner) };
    registry.read_audio(source_token("a"), &mut [0.0; 256], 128, 48_000);
    assert!(registry.set_clock_master(source_token("a"), true));
    (registry, transport, mapping)
}

#[test]
fn consumer_publication_during_converter_setup_keeps_the_new_epoch_and_pcm() {
    let (registry, mut transport, mapping) = source();
    let before = transport.clock().unwrap();
    // SAFETY: this fixture is the only consumer and retains the independent mapping.
    let consumer = unsafe { &*mapping.clock };
    consumer.consumer_sequence.fetch_add(1, Ordering::AcqRel);
    assert_eq!(transport.observe_clock(), SourceClockRead::Deferred);
    let mut converter = Converter::new(before, (48_000, 2)).unwrap();
    assert!(
        converter.restart(&mut transport, before),
        "producer reset is independent of the busy consumer snapshot"
    );
    assert_eq!(transport.observe_clock(), SourceClockRead::Deferred);
    consumer.consumer_sequence.fetch_add(1, Ordering::Release);
    let after = transport.clock().unwrap();
    assert_ne!(after.epoch, before.epoch);
    assert_eq!(after.epoch, converter.epoch);
    let mut input = [0.25; 256];
    let times = std::array::from_fn::<_, 128, _>(|frame| {
        1_000_000_000 + frame as u64 * 1_000_000_000 / 48_000
    });
    // SAFETY: the independently retained grant has immutable, validated ring geometry.
    let buffer = unsafe {
        mixrelay_macos_transport::shared_audio_buffer::mr_shared_audio_buffer_attach(
            mapping.address,
            mapping.bytes,
        )
    };
    assert!(!buffer.is_null());
    let mut published = [0.0; 256];
    let measured = allocation_counter::measure(|| {
        let mut offset = 0;
        for frames in [21, 5, 23, 79] {
            converter
                .push(
                    &mut transport,
                    &mut input[..frames * 2],
                    &times[offset..offset + frames],
                )
                .unwrap();
            assert!(converter.flush(&mut transport).unwrap());
            offset += frames;
            let committed = registry.clock(source_token("a")).unwrap();
            // Converter priming may retain input. Only PCM actually readable from
            // the shared ring may advance the graph's clock, regardless of fragment size.
            // SAFETY: the ring is retained and the exclusive slice holds this complete range.
            let supplied = unsafe {
                mixrelay_macos_transport::shared_audio_buffer::mr_shared_audio_buffer_read(
                    buffer,
                    0,
                    published.as_mut_ptr(),
                    u32::try_from(offset).unwrap(),
                )
            };
            assert_eq!(committed.native_frames, u64::from(supplied));
            assert!(
                published[..supplied as usize * 2]
                    .iter()
                    .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
            );
        }
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(transport.clock().unwrap().produced_frames, 128);
    let committed = registry.clock(source_token("a")).unwrap();
    assert_eq!(committed.native_frames, 128);
    assert_eq!(committed.running, 1);
}

/// While a time-addressed consumer is mid-publication its timing is unreadable. The
/// converter keeps the correction it measured instead of falling back for that block
/// to queue occupancy, which carries no drift for such a consumer.
#[test]
fn an_unreadable_time_addressed_consumer_keeps_its_correction() {
    let (registry, mut transport, mapping) = source();
    assert!(registry.set_clock_master(source_token("a"), false));
    let clock = transport.clock().unwrap();
    let mut converter = Converter::new(clock, (48_000, 2)).unwrap();
    assert!(converter.restart(&mut transport, clock));
    let clock = transport.clock().unwrap();
    // SAFETY: this fixture is the only consumer and retains the independent mapping.
    let consumer = unsafe { &*mapping.clock };
    consumer.publish_consumer(
        mixrelay_macos_transport::application_clock::ConsumerPublication {
            epoch: clock.epoch,
            next_frame: 0,
            frames: 128,
            running: true,
            target_backlog_frames: 256,
            phase_ns: 0,
            // A device 100 ppm fast asks for 100 ppm more frames.
            period_ns: 1e9 / 48_000.0 * (1.0 - 100e-6),
        },
    );
    let clock = transport.clock().unwrap();
    converter.observe(&transport, clock).unwrap();
    let measured = converter.correction_ppm();
    assert!((measured - 100.0).abs() < 1.0, "measured {measured} ppm");
    consumer.consumer_sequence.fetch_add(1, Ordering::AcqRel);
    // An empty queue against its target would steer occupancy feedback away at once.
    let starved = SourceClockSnapshot {
        consumer_ticks: clock.consumer_ticks + 128,
        consumer_running: 1,
        target_backlog_frames: 4_096,
        produced_frames: 0,
        consumed_frames: 0,
        ..clock
    };
    converter.observe(&transport, starved).unwrap();
    assert_eq!(converter.correction_ppm().to_bits(), measured.to_bits());
    consumer.consumer_sequence.fetch_add(1, Ordering::Release);
}
