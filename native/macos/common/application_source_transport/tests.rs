use super::ffi::{
    TransportError, TransportErrorCode, mr_application_source_transport_attach,
    mr_application_source_transport_destroy, mr_application_source_transport_write,
};
use super::*;
use crate::{
    application_clock::{mr_application_clock_publish_consumer, mr_application_clock_request_rate},
    create_shared_memory,
    shared_audio_buffer::mr_shared_audio_buffer_try_begin_epoch,
    source_control::SourceAttachment,
};
use std::{
    os::fd::{AsRawFd, IntoRawFd},
    ptr,
};

fn fixture() -> (Mapping, SourceTransport) {
    let mapping = Mapping::new(1024, 2).unwrap();
    mapping.clock().producer_lease.store(7, Ordering::Release);
    let epoch = unsafe { mr_shared_audio_buffer_stats(mapping.buffer.as_ptr()) }.epoch;
    let mut attachment = SourceAttachment {
        descriptor: mapping.descriptor.try_clone().unwrap().into_raw_fd(),
        mapped_size: mapping.bytes,
        producer_lease: 7,
        source_epoch: epoch,
    };
    let pointer = unsafe {
        mr_application_source_transport_attach(&raw mut attachment, 1024, 2, ptr::null_mut())
    };
    assert!(!pointer.is_null());
    assert_eq!(attachment.descriptor, -1);
    // Recover the unique native owner without changing the production allocation path.
    (mapping, *unsafe { Box::from_raw(pointer) })
}

#[test]
fn source_faults_survive_busy_claims_and_epoch_reset_but_never_cross_stale_leases() {
    let (mapping, mut producer) = fixture();
    let format = unsafe { mr_application_clock_request_rate(mapping.clock.as_ptr(), 48_000) };
    assert!(producer.reset(format, 48_000));
    let old_epoch = producer.owned_epoch;
    let claim = ProducerClaim::acquire(mapping.clock()).unwrap();
    let measured = allocation_counter::measure(|| {
        assert!(producer.record_source_faults(old_epoch, 3));
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(mapping.clock().source_sample_faults.take(), 0);
    assert_eq!(producer.pending_source_faults, 3);
    drop(claim);
    assert!(producer.reset(format, 44_100));
    assert_ne!(producer.owned_epoch, old_epoch);
    assert_eq!(mapping.clock().source_sample_faults.take(), 3);
    assert!(!producer.record_source_faults(old_epoch, 99));
    assert!(producer.record_source_faults(producer.owned_epoch, 2));
    assert_eq!(mapping.clock().source_sample_faults.take(), 2);
    assert!(producer.record_source_faults(producer.owned_epoch, 0));
    assert_eq!(mapping.clock().source_sample_faults.take(), 0);
    mapping.clock().producer_lease.store(8, Ordering::Release);
    assert!(!producer.record_source_faults(producer.owned_epoch, 99));
    drop(producer);
    assert_eq!(mapping.clock().source_sample_faults.take(), 0);
}

#[test]
fn pending_faults_flush_on_empty_retry_write_and_drop_without_duplicate_reporting() {
    for flush in 0..3 {
        let (mapping, mut producer) = fixture();
        let format = unsafe { mr_application_clock_request_rate(mapping.clock.as_ptr(), 48_000) };
        assert!(producer.reset(format, 48_000));
        let claim = ProducerClaim::acquire(mapping.clock()).unwrap();
        assert!(producer.record_source_faults(producer.owned_epoch, 3));
        drop(claim);
        match flush {
            0 => assert!(producer.record_source_faults(producer.owned_epoch, 0)),
            1 => assert!(producer.write(&[0.25; 256], 128)),
            _ => drop(producer),
        }
        assert_eq!(mapping.clock().source_sample_faults.take(), 3);
        assert_eq!(mapping.clock().source_sample_faults.take(), 0);
    }
}

#[test]
fn revoked_epoch_or_lease_cannot_flush_previously_queued_normalization_into_replacement() {
    for replace_lease in [false, true] {
        let (mapping, mut producer) = fixture();
        let format = unsafe { mr_application_clock_request_rate(mapping.clock.as_ptr(), 48_000) };
        assert!(producer.reset(format, 48_000));
        let claim = ProducerClaim::acquire(mapping.clock()).unwrap();
        assert!(producer.record_source_faults(producer.owned_epoch, 3));
        if replace_lease {
            mapping.clock().producer_lease.store(8, Ordering::Release);
        } else {
            // SAFETY: this test owns the mapping and deliberately revokes the epoch.
            unsafe {
                crate::shared_audio_buffer::mr_shared_audio_buffer_request_epoch(
                    mapping.buffer.as_ptr(),
                )
            };
        }
        drop(claim);
        assert!(!producer.record_source_faults(producer.owned_epoch, 0));
        assert!(!producer.reset(format, 48_000));
        drop(producer);
        assert_eq!(mapping.clock().source_sample_faults.take(), 0);
    }
    // SAFETY: null is an explicit rejected boundary input.
    assert!(!unsafe {
        mr_application_source_transport_record_source_faults(ptr::null_mut(), 1, 3)
    });
}

#[test]
fn reset_write_snapshot_and_stale_lease_use_the_actual_mapping() {
    let (mapping, mut producer) = fixture();
    let format = unsafe { mr_application_clock_request_rate(mapping.clock.as_ptr(), 44_100) };
    assert!(!producer.write(&[0.25; 256], 128));
    assert!(producer.reset(format, 48_000));
    assert!(producer.write(&[0.25; 256], 128));
    producer.publish_native(256, true);
    unsafe {
        mr_application_clock_publish_consumer(
            mapping.clock.as_ptr(),
            producer.owned_epoch,
            64,
            64,
            true,
            384,
        );
    };
    let snapshot = producer.clock().unwrap();
    assert_eq!(
        (snapshot.produced_frames, snapshot.consumed_frames),
        (128, 64)
    );
    assert_eq!(mapping.clock().native_frames.load(Ordering::Acquire), 256);
    mapping.clock().producer_lease.store(8, Ordering::Release);
    assert!(producer.clock().is_none());
    assert!(!producer.reset(format, 48_000));
    assert!(!producer.write(&[0.5; 256], 128));
    drop(producer);
    // A retired producer's destructor must not stop the replacement's native clock.
    assert_eq!(mapping.clock().producer_lease.load(Ordering::Acquire), 8);
    assert_eq!(mapping.clock().native_running.load(Ordering::Acquire), 1);
    assert_eq!(mapping.clock().native_frames.load(Ordering::Acquire), 256);
}

#[test]
fn drop_closes_current_lease_even_while_a_remote_grant_holds_the_guard() {
    let (mapping, producer) = fixture();
    producer.publish_native(128, true);
    let claim = ProducerClaim::acquire(mapping.clock()).unwrap();
    drop(producer);
    assert_eq!(mapping.clock().producer_lease.load(Ordering::Acquire), 0);
    // Without the guard, Drop leaves optional metadata untouched. Readers reject
    // the closed lease regardless, and a successful grant may replace this tuple.
    assert_eq!(mapping.clock().native_running.load(Ordering::Acquire), 1);
    drop(claim);
}

#[test]
fn drop_closes_current_lease_after_epoch_invalidation() {
    let (mapping, producer) = fixture();
    unsafe {
        crate::shared_audio_buffer::mr_shared_audio_buffer_request_epoch(mapping.buffer.as_ptr());
    }
    assert_eq!(
        unsafe { mr_shared_audio_buffer_stats(mapping.buffer.as_ptr()) }.epoch,
        0
    );
    assert!(!producer.owns_stream());
    drop(producer);
    assert_eq!(mapping.clock().producer_lease.load(Ordering::Acquire), 0);
    assert_eq!(
        mapping.clock().acknowledged_format.load(Ordering::Acquire),
        0
    );
}

#[test]
fn reset_exhaustion_keeps_local_cursor_and_releases_producer_claim() {
    let (mapping, mut producer) = fixture();
    for _ in producer.owned_epoch..u64::from(u16::MAX) {
        producer.owned_epoch =
            unsafe { mr_shared_audio_buffer_try_begin_epoch(mapping.buffer.as_ptr()) };
    }
    producer.next_frame = 123;
    let format = unsafe { mr_application_clock_request_rate(mapping.clock.as_ptr(), 48_000) };
    mapping
        .clock()
        .acknowledged_format
        .store(format, Ordering::Release);
    mapping.clock().native_running.store(1, Ordering::Release);
    mapping
        .clock()
        .native_sample_rate
        .store(44_100, Ordering::Release);
    assert!(!producer.reset(format, 48_000));
    assert_eq!(
        (producer.owned_epoch, producer.next_frame),
        (u64::from(u16::MAX), 123)
    );
    assert_eq!(
        mapping.clock().acknowledged_format.load(Ordering::Acquire),
        0
    );
    assert_eq!(mapping.clock().native_running.load(Ordering::Acquire), 0);
    assert_eq!(
        mapping.clock().native_sample_rate.load(Ordering::Acquire),
        44_100
    );
    assert!(ProducerClaim::acquire(mapping.clock()).is_some());
}

#[test]
fn busy_producer_defers_and_consumer_publication_does_not_revoke_the_grant() {
    let (mapping, mut producer) = fixture();
    let format = unsafe { mr_application_clock_request_rate(mapping.clock.as_ptr(), 48_000) };
    let claim = ProducerClaim::acquire(mapping.clock()).unwrap();
    assert!(!producer.reset(format, 48_000));
    assert!(!producer.write(&[0.25; 2], 1));
    producer.publish_native(9, true);
    assert_eq!(producer.next_frame, 0);
    assert_eq!(mapping.clock().native_frames.load(Ordering::Acquire), 0);
    drop(claim);
    assert!(producer.reset(format, 48_000));
    mapping
        .clock()
        .consumer_sequence
        .fetch_add(1, Ordering::AcqRel);
    assert!(producer.clock().is_none());
    assert_eq!(producer.observe_clock(), SourceClockRead::Deferred);
    let previous = producer.owned_epoch;
    let epoch = producer.begin_epoch(format, 48_000).unwrap();
    assert_ne!(epoch, previous);
    assert_eq!(producer.observe_clock(), SourceClockRead::Deferred);
    assert!(producer.record_source_faults(epoch, 0));
    assert!(producer.write(&[0.25; 256], 128));
    mapping
        .clock()
        .consumer_sequence
        .fetch_add(1, Ordering::Release);
    let SourceClockRead::Ready(snapshot) = producer.observe_clock() else {
        panic!("consumer publication must become observable after commit")
    };
    assert_eq!(snapshot.epoch, epoch);
    assert_eq!(snapshot.produced_frames, 128);
    mapping.clock().producer_lease.store(8, Ordering::Release);
    assert_eq!(producer.observe_clock(), SourceClockRead::Revoked);
}

#[test]
fn attachment_rejects_short_storage_and_clears_descriptor_on_failure() {
    let descriptor = create_shared_memory(64, true).unwrap();
    let mut attachment = SourceAttachment {
        descriptor: descriptor.into_raw_fd(),
        mapped_size: crate::application_clock::mr_application_clock_required_bytes(1024, 2),
        producer_lease: 7,
        source_epoch: 1,
    };
    let mut error = TransportError::default();
    assert!(
        unsafe {
            mr_application_source_transport_attach(&raw mut attachment, 1024, 2, &raw mut error)
        }
        .is_null()
    );
    assert_eq!(error.code, TransportErrorCode::InvalidAttachment);
    assert_eq!(attachment.descriptor, -1);
    assert_eq!(attachment.mapped_size, 0);
}

#[test]
fn attachment_rejects_wrong_geometry_and_stale_grants() {
    let (mapping, producer) = fixture();
    for (channels, lease, epoch) in [
        (1, 7, producer.owned_epoch),
        (2, 8, producer.owned_epoch),
        (2, 7, 0),
    ] {
        let mut attachment = SourceAttachment {
            descriptor: mapping.descriptor.try_clone().unwrap().into_raw_fd(),
            mapped_size: mapping.bytes,
            producer_lease: lease,
            source_epoch: epoch,
        };
        let mut error = TransportError::default();
        assert!(
            unsafe {
                mr_application_source_transport_attach(
                    &raw mut attachment,
                    1024,
                    channels,
                    &raw mut error,
                )
            }
            .is_null()
        );
        assert_eq!(error.code, TransportErrorCode::InvalidAttachment);
        assert_eq!(attachment.descriptor, -1);
    }
}

#[test]
fn allocation_failure_retires_owner_and_reports_enomem() {
    let (mapping, producer) = fixture();
    producer.publish_native(1, true);
    let mut error = TransportError::default();
    let pointer = unsafe { ffi::publish_owner(Ok(producer), &raw mut error, |_| ptr::null_mut()) };
    assert!(pointer.is_null());
    assert_eq!(error.code, TransportErrorCode::MappingFailed);
    assert_eq!(error.system_error, 12);
    assert_eq!(mapping.clock().native_running.load(Ordering::Acquire), 0);
    assert_eq!(mapping.clock().producer_lease.load(Ordering::Acquire), 0);
}

#[test]
fn callbacks_allocate_nothing_and_reject_invalid_input_extents() {
    let (mapping, mut producer) = fixture();
    let format = unsafe { mr_application_clock_request_rate(mapping.clock.as_ptr(), 48_000) };
    assert!(producer.reset(format, 48_000));
    let input = [0.25; 64];
    let observed = allocation_counter::measure(|| {
        for _ in 0..128 {
            assert!(producer.write(&input, 32));
            producer.publish_native(producer.next_frame, true);
            assert!(producer.clock().is_some());
        }
    });
    assert_eq!(observed.count_total, 0);
    assert!(!producer.write(&input[..1], 1));
    assert!(!unsafe { mr_application_source_transport_write(&raw mut producer, ptr::null(), 1) });
    assert!(!unsafe {
        mr_application_source_transport_write(&raw mut producer, input.as_ptr(), 1025)
    });
    unsafe { mr_application_source_transport_destroy(ptr::null_mut()) };
}

#[test]
fn retirement_unmaps_and_closes_storage_in_an_isolated_process() {
    unsafe extern "C" {
        fn mprotect(address: *mut std::ffi::c_void, length: usize, protection: i32) -> i32;
        fn fcntl(descriptor: i32, command: i32, ...) -> i32;
    }
    const CHILD: &str = "MIXRELAY_SOURCE_RETIREMENT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "application_source_transport::tests::retirement_unmaps_and_closes_storage_in_an_isolated_process", "--test-threads=1"])
            .env(CHILD, "1").status().unwrap();
        assert!(status.success());
        return;
    }
    // No parallel test may reuse a retired descriptor or virtual address between
    // drop and these OS observations; this child runs only this one fixture.
    let (_mapping, producer) = fixture();
    let address = producer.mapping.buffer.as_ptr().cast();
    let descriptor = producer.mapping.descriptor.as_raw_fd();
    assert_eq!(unsafe { mprotect(address, 1, 3) }, 0);
    assert_ne!(unsafe { fcntl(descriptor, 1) }, -1);
    drop(producer);
    // Darwin mincore does not reject unmapped pages; unchanged RW protection
    // verifies that this first mapped page exists before drop and not afterward.
    assert_eq!(unsafe { mprotect(address, 1, 3) }, -1);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(12));
    assert_eq!(unsafe { fcntl(descriptor, 1) }, -1);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(9));

    let owned = create_shared_memory(64, true).unwrap();
    let descriptor = owned.into_raw_fd();
    let mut attachment = SourceAttachment {
        descriptor,
        mapped_size: 64,
        producer_lease: 7,
        source_epoch: 1,
    };
    assert!(
        unsafe {
            mr_application_source_transport_attach(&raw mut attachment, 1024, 2, ptr::null_mut())
        }
        .is_null()
    );
    assert_eq!(unsafe { fcntl(descriptor, 1) }, -1);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(9));
}

#[test]
fn measured_bursts_are_epoch_scoped_and_closed_or_busy_leases_cannot_publish() {
    let (mapping, mut producer) = fixture();
    let format = unsafe { mr_application_clock_request_rate(mapping.clock.as_ptr(), 48_000) };
    assert!(producer.reset(format, 44_100));
    unsafe {
        mr_application_source_transport_publish_measurements(ptr::null(), 512, 128, 64);
    }
    producer.publish_measurements(1024, 140, 50);
    producer.publish_measurements(128, 128, 1);
    let clock = mapping.clock();
    assert_eq!(clock.producer_burst_frames.load(Ordering::Acquire), 1024);
    assert_eq!(clock.converter_burst_frames.load(Ordering::Acquire), 140);
    assert_eq!(clock.publication_drought_frames.load(Ordering::Acquire), 50);
    let claim = ProducerClaim::acquire(clock).unwrap();
    producer.publish_measurements(4096, 4096, 4096);
    drop(claim);
    assert_eq!(clock.producer_burst_frames.load(Ordering::Acquire), 1024);
    assert!(producer.reset(format, 44_100));
    assert_eq!(clock.producer_burst_frames.load(Ordering::Acquire), 0);
    assert_eq!(clock.converter_burst_frames.load(Ordering::Acquire), 0);
    assert_eq!(clock.publication_drought_frames.load(Ordering::Acquire), 0);
    unsafe {
        mr_application_clock_publish_consumer(
            mapping.clock.as_ptr(),
            producer.owned_epoch,
            42,
            128,
            false,
            768,
        );
    }
    assert_eq!(producer.clock().unwrap().target_backlog_frames, 768);
    assert!(producer.reset(format, 44_100));
    let stale = producer.clock().unwrap();
    assert_eq!(
        (
            stale.consumed_frames,
            stale.consumer_running,
            stale.target_backlog_frames
        ),
        (0, 0, 0)
    );
    clock.producer_lease.store(0, Ordering::Release);
    producer.publish_measurements(4096, 4096, 4096);
    assert_eq!(clock.producer_burst_frames.load(Ordering::Acquire), 0);
}

#[test]
fn concurrent_target_and_cursor_snapshots_are_coherent() {
    let (mapping, producer) = fixture();
    assert_eq!(std::mem::size_of::<SourceClockSnapshot>(), 96);
    let clock = mapping.clock();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for target in 1..=4000 {
                unsafe {
                    mr_application_clock_publish_consumer(
                        std::ptr::from_ref(clock).cast_mut(),
                        producer.owned_epoch,
                        u64::from(target) * 2,
                        1,
                        true,
                        target,
                    );
                }
            }
        });
        for _ in 0..10000 {
            if let Some(snapshot) = producer.clock() {
                assert_eq!(
                    snapshot.consumed_frames,
                    u64::from(snapshot.target_backlog_frames) * 2
                );
            }
        }
    });
}

#[test]
fn clocked_writes_publish_only_committed_pcm_credit_and_stop_preserves_position() {
    let (mapping, mut producer) = fixture();
    // SAFETY: the fixture owns this initialized clock and serializes its consumer.
    let format = unsafe { mr_application_clock_request_rate(mapping.clock.as_ptr(), 48_000) };
    assert!(producer.begin_epoch(format, 24_000).is_some());
    assert_eq!(mapping.clock().native_frames.load(Ordering::Acquire), 0);
    let measured = allocation_counter::measure(|| {
        // SAFETY: this fixture owns the producer and complete stereo PCM/timestamp spans.
        assert!(unsafe {
            mr_application_source_transport_write_clocked(
                &raw mut producer,
                [0.25; 48].as_ptr(),
                [100; 24].as_ptr(),
                24,
            )
        });
        assert_eq!(mapping.clock().native_frames.load(Ordering::Acquire), 12);
        // SAFETY: the sole producer and both immutable input spans remain live for this call.
        assert!(unsafe {
            mr_application_source_transport_write_clocked(
                &raw mut producer,
                [0.0; 32].as_ptr(),
                [200; 16].as_ptr(),
                16,
            )
        });
        assert_eq!(mapping.clock().native_frames.load(Ordering::Acquire), 20);
        let claim = ProducerClaim::acquire(mapping.clock()).unwrap();
        assert!(!producer.write_clocked_pcm(&[0.5; 16], 8, &[300; 8]));
        assert_eq!(mapping.clock().native_frames.load(Ordering::Acquire), 20);
        assert_eq!(producer.next_frame, 40);
        drop(claim);
        let mut pcm = [9.0; 80];
        // SAFETY: this retained ring has two channels and the complete exclusive destination.
        assert_eq!(
            unsafe {
                crate::shared_audio_buffer::mr_shared_audio_buffer_read(
                    mapping.buffer.as_ptr(),
                    0,
                    pcm.as_mut_ptr(),
                    40,
                )
            },
            40
        );
        assert!(
            pcm[..48]
                .iter()
                .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
        );
        assert!(
            pcm[48..]
                .iter()
                .all(|sample| sample.to_bits() == 0.0_f32.to_bits())
        );
        producer.stop_native();
        assert_eq!(mapping.clock().native_frames.load(Ordering::Acquire), 20);
        assert_eq!(mapping.clock().native_running.load(Ordering::Acquire), 0);
        assert!(producer.write_clocked_pcm(&[], 0, &[]));
        assert_eq!(mapping.clock().native_running.load(Ordering::Acquire), 0);
        assert!(producer.begin_epoch(format, 96_000).is_some());
        assert_eq!(mapping.clock().native_frames.load(Ordering::Acquire), 0);
        assert_eq!(
            mapping.clock().native_sample_rate.load(Ordering::Acquire),
            96_000
        );
        assert_eq!(mapping.clock().native_running.load(Ordering::Acquire), 0);
        assert!(producer.write_clocked_pcm(&[0.375; 4], 2, &[400; 2]));
        assert_eq!(mapping.clock().native_frames.load(Ordering::Acquire), 4);
    });
    assert_eq!(measured.count_total, 0);
}

#[test]
fn failed_local_attachment_reclaims_its_registry_mapping_and_can_retry() {
    use crate::{application_clock::ConsumerRoute, source_control::SourceRequest};
    let registry = local_registry();
    let mut request = SourceRequest {
        channels: 2,
        capacity_frames: 1024,
        transport_version: crate::application_clock::VERSION,
        ..SourceRequest::default()
    };
    for (slot, byte) in request.source_id.iter_mut().zip(b"local-attachment") {
        *slot = std::ffi::c_char::from_ne_bytes([*byte]);
    }
    // SAFETY: the static string is terminated and readable for the token calculation.
    let token =
        unsafe { crate::source_registry::mr_engine_source_id_token(c"local-attachment".as_ptr()) };
    let failed = SourceTransport::create_attaching(&registry, &request, |raw| {
        raw.mapped_size = 0;
        // SAFETY: attach consumes this unique real descriptor even when its extent fails.
        unsafe { super::ffi::attach(raw, 1024, 2) }
    });
    assert!(failed.is_err());
    assert!(!registry.set_consumer_route(token, ConsumerRoute::Unrouted));
    let producer = SourceTransport::create(&registry, &request).unwrap();
    assert!(registry.set_consumer_route(token, ConsumerRoute::Unrouted));
    drop(producer);
    registry.collect_retired();
    assert!(!registry.set_consumer_route(token, ConsumerRoute::Unrouted));
}

fn local_registry() -> crate::source_registry::SourceRegistry {
    use crate::source_registry::SourceRegistry;
    use mixrelay_time::{Instant, MonotonicClock};
    struct Clock(Instant);
    impl MonotonicClock for Clock {
        fn now(&self) -> Instant {
            self.0
        }
    }
    fn storage(bytes: usize) -> std::io::Result<std::os::fd::OwnedFd> {
        let directory = mixrelay_test_support::FixtureDir::new("src-local")?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.path().join("pcm"))?;
        file.set_len(u64::try_from(bytes).unwrap())?;
        Ok(file.into())
    }
    SourceRegistry::with_clock_and_storage(
        Box::new(Clock(Instant::from_origin(std::time::Duration::ZERO))),
        mixrelay_realtime::Timebase::from_ratio(
            std::num::NonZeroU32::new(1).unwrap(),
            std::num::NonZeroU32::new(1).unwrap(),
        ),
        storage,
    )
}

#[test]
fn prepared_producer_detects_terminal_storage_before_the_next_stream() {
    use crate::source_control::SourceRequest;
    let registry = local_registry();
    let mut request = SourceRequest {
        channels: 2,
        capacity_frames: 1024,
        transport_version: crate::application_clock::VERSION,
        ..SourceRequest::default()
    };
    for (slot, byte) in request.source_id.iter_mut().zip(b"terminal-local") {
        *slot = std::ffi::c_char::from_ne_bytes([*byte]);
    }
    // SAFETY: the static source ID is terminated and readable throughout the call.
    let token =
        unsafe { crate::source_registry::mr_engine_source_id_token(c"terminal-local".as_ptr()) };
    let producer = SourceTransport::create(&registry, &request).unwrap();
    assert!(registry.producer_can_rearm(token));
    producer
        .mapping
        .clock()
        .format_word
        .store(u64::MAX, Ordering::Release);
    assert!(!registry.producer_can_rearm(token));
    let replacement = SourceTransport::create(&registry, &request).unwrap();
    assert!(registry.producer_can_rearm(token));
    assert_eq!(producer.observe_clock(), SourceClockRead::Revoked);
    assert_ne!(replacement.observe_clock(), SourceClockRead::Revoked);
}
