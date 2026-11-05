//! Attach, replacement and retirement of producer mappings.
use super::*;

#[test]
fn replacement_preserves_published_faults_exactly_once_with_a_pinned_old_reader() {
    for order in 0..3 {
        let registry = registry();
        let request = request("fault-replacement");
        registry.attach(&request).unwrap();
        let token = token(b"fault-replacement");
        let old_read = registry.read(token);
        let old = old_read.source().unwrap();
        old.mapping.clock().source_sample_faults.record(3);
        old.mapping
            .clock()
            .format_word
            .store((u64::from(u32::MAX) << 32) + 48_000, Ordering::Release);
        assert!(old.terminal());
        let old_observed = match order {
            0 => {
                let observed = old.mapping.clock().source_sample_faults.take();
                registry.attach(&request).unwrap();
                observed
            }
            1 => {
                registry.attach(&request).unwrap();
                old.mapping.clock().source_sample_faults.take()
            }
            _ => {
                let barrier = std::sync::Barrier::new(2);
                std::thread::scope(|scope| {
                    let reader = scope.spawn(|| {
                        barrier.wait();
                        old.mapping.clock().source_sample_faults.take()
                    });
                    barrier.wait();
                    registry.attach(&request).unwrap();
                    reader.join().unwrap()
                })
            }
        };
        let current = registry.read(token);
        assert!(!std::ptr::eq(current.source().unwrap(), old));
        assert_eq!(old_observed + registry.take_source_faults(token), 3);
        assert_eq!(registry.take_source_faults(token), 0);
        assert_eq!(old.mapping.clock().source_sample_faults.take(), 0);
    }
}

fn exhaust(source: &Source) {
    let buffer = source.mapping.buffer.as_ptr();
    let epoch = unsafe { mr_shared_audio_buffer_stats(buffer) }.epoch;
    for expected in epoch + 1..=u64::from(u16::MAX) {
        assert_eq!(
            unsafe { mr_shared_audio_buffer_try_begin_epoch(buffer) },
            expected
        );
    }
    assert_eq!(unsafe { mr_shared_audio_buffer_try_begin_epoch(buffer) }, 0);
    assert!(source.terminal());
}

#[test]
fn replacing_an_active_max_epoch_revokes_old_pcm_before_publication() {
    let registry = registry();
    let request = request("active-max");
    registry.attach(&request).unwrap();
    let old = registry.read(token(b"active-max"));
    let buffer = old.source().unwrap().mapping.buffer.as_ptr();
    // SAFETY: the read guard pins this test-only mapping and the test is its
    // sole producer. Deliberately stop at active MAX, before a rearm request.
    unsafe {
        for _ in 2..u16::MAX {
            assert_ne!(mr_shared_audio_buffer_try_begin_epoch(buffer), 0);
        }
        assert_eq!(
            mr_shared_audio_buffer_stats(buffer).epoch,
            u64::from(u16::MAX)
        );
        assert!(!mr_shared_audio_buffer_requires_replacement(buffer));
    }
    registry.attach(&request).unwrap();
    unsafe {
        assert!(mr_shared_audio_buffer_requires_replacement(buffer));
        assert!(!mr_shared_audio_buffer_write(
            buffer,
            u64::from(u16::MAX),
            0,
            [0.5; 2].as_ptr(),
            1
        ));
    }
}

#[test]
fn exhausted_source_replaces_mapping_preserves_settings_and_pins_old_reader() {
    let registry = registry();
    let request = request("application.one");
    let first = registry.attach(&request).unwrap();
    let read = registry.read(token(b"application.one"));
    let old = read.source().unwrap();
    unsafe {
        mr_application_clock_request_rate(old.mapping.clock.as_ptr(), 96_000);
    }
    old.mapping.clock().clock_master.store(1, Ordering::Release);
    exhaust(old);
    let second = registry.attach(&request).unwrap();
    assert!(second.producer_lease > first.producer_lease);
    assert_eq!(second.source_epoch, 2);
    let current = registry.read(token(b"application.one"));
    let current = current.source().unwrap();
    assert_ne!(old.mapping.buffer, current.mapping.buffer);
    assert_eq!(
        current.mapping.clock().format_word.load(Ordering::Acquire) & u64::from(u32::MAX),
        96_000
    );
    assert_eq!(
        current.mapping.clock().clock_master.load(Ordering::Acquire),
        1
    );
    assert_eq!(
        old.mapping.clock().producer_lease.load(Ordering::Acquire),
        0
    );
    assert_eq!(registry.control().owners.len(), 2);
    assert!(!unsafe {
        mr_shared_audio_buffer_write(
            old.mapping.buffer.as_ptr(),
            first.source_epoch,
            0,
            [0.25; 2].as_ptr(),
            1,
        )
    });
    assert!(unsafe {
        mr_shared_audio_buffer_write(
            current.mapping.buffer.as_ptr(),
            second.source_epoch,
            0,
            [0.5; 2].as_ptr(),
            1,
        )
    });
    drop(read);
    // The second read also retains owners until its scope ends; reclamation
    // only occurs on a later control call, never on either reader's Drop.
}

#[test]
fn attachment_descriptor_outlives_registry_and_does_not_alias_other_sources() {
    let registry = registry();
    let first = registry.attach(&request("one")).unwrap();
    let second = registry.attach(&request("two")).unwrap();
    let first_pointer = registry
        .read(token(b"one"))
        .source()
        .unwrap()
        .mapping
        .buffer;
    let second_pointer = registry
        .read(token(b"two"))
        .source()
        .unwrap()
        .mapping
        .buffer;
    assert_ne!(first_pointer, second_pointer);
    drop(registry);
    let first_file = std::fs::File::from(first.descriptor);
    let second_file = std::fs::File::from(second.descriptor);
    assert!(first_file.metadata().unwrap().len() >= first.mapped_size as u64);
    assert!(second_file.metadata().unwrap().len() >= second.mapped_size as u64);
}

#[test]
fn busy_or_invalid_attachment_preserves_active_lease_and_mapping() {
    let registry = registry();
    let request = request("one");
    let attachment = registry.attach(&request).unwrap();
    let read = registry.read(token(b"one"));
    let source = read.source().unwrap();
    let producer = source.claim_producer().unwrap();
    assert_eq!(
        registry.attach(&request).err().unwrap().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(
        source
            .mapping
            .clock()
            .producer_lease
            .load(Ordering::Acquire),
        attachment.producer_lease
    );
    drop(producer);
    let invalid = SourceRequest {
        transport_version: 0,
        ..request
    };
    assert!(registry.attach(&invalid).is_err());
    assert_eq!(registry.control().owners.len(), 1);
}

#[test]
fn sequential_producer_drop_reuses_slots_beyond_the_active_source_limit() {
    let registry = registry();
    for index in 0..MAX_SOURCES * 2 {
        let owner = producer(&registry, &request(&format!("application.{index}")));
        assert_eq!(registry.control().owners.len(), 1);
        drop(owner);
    }
    // Each subsequent real attach collects its predecessor; the final closed
    // source is reclaimed at the next control collection, not from its Drop.
    collect(&registry);
    assert!(registry.control().owners.is_empty());
    assert!(registry.slots.is_empty());
}

#[test]
fn paused_producers_retain_all_64_active_slots() {
    let registry = registry();
    let owners: Vec<_> = (0..MAX_SOURCES)
        .map(|index| producer(&registry, &request(&format!("paused.{index}"))))
        .collect();
    for owner in &owners {
        unsafe {
            mr_application_source_transport_publish_native(&raw const *owner, 128, true);
            mr_application_source_transport_publish_native(&raw const *owner, 0, false);
        }
    }
    collect(&registry);
    assert_eq!(registry.control().owners.len(), MAX_SOURCES);
    assert_eq!(
        registry
            .attach(&request("overflow"))
            .err()
            .unwrap()
            .to_string(),
        "source inventory full"
    );
    drop(owners);
    collect(&registry);
    assert!(registry.control().owners.is_empty());
    drop(producer(&registry, &request("overflow")));
}

#[test]
fn closed_source_is_silent_before_collection_and_same_id_gets_a_new_lease() {
    let registry = registry();
    let request = request("one");
    let mut owner = producer(&registry, &request);
    let read = registry.read(token(b"one"));
    let source = read.source().unwrap();
    let clock = source.mapping.clock();
    let old_lease = clock.producer_lease.load(Ordering::Acquire);
    let mut output = [9.0; 256];
    assert_eq!(
        registry.read_audio(source.token, &mut output, 128, 48_000),
        0
    );
    let format = clock.format_word.load(Ordering::Acquire);
    unsafe {
        assert!(mr_application_source_transport_reset(
            &raw mut owner,
            format,
            48_000
        ));
        assert!(mr_application_source_transport_write(
            &raw mut owner,
            [0.25; 1536].as_ptr(),
            768
        ));
        mr_application_source_transport_publish_native(&raw const owner, 768, true);
    }
    assert_eq!(
        registry.read_audio(source.token, &mut output, 128, 48_000),
        128
    );
    let epoch = unsafe { mr_shared_audio_buffer_stats(source.mapping.buffer.as_ptr()) }.epoch;
    assert!(source.read_is_current(old_lease, format, epoch));
    let grant_claim = source.claim_producer().unwrap();
    drop(owner);
    // PCM is already copied. Even when guard contention leaves its old format
    // and epoch intact, the exact post-read predicate must reject the closed lease.
    assert_eq!(clock.acknowledged_format.load(Ordering::Acquire), format);
    assert_eq!(
        unsafe { mr_shared_audio_buffer_stats(source.mapping.buffer.as_ptr()) }.epoch,
        epoch
    );
    assert!(!source.read_is_current(old_lease, format, epoch));
    drop(grant_claim);
    let observed = allocation_counter::measure(|| {
        assert_eq!(
            registry.read_audio(source.token, &mut output, 128, 48_000),
            0
        );
    });
    assert_eq!(observed.count_total, 0);
    assert!(output.iter().all(|sample| *sample == 0.0));
    assert!(registry.clock(source.token).is_none());
    collect(&registry);
    assert!(registry.read(source.token).source().is_none());
    assert_eq!(registry.control().owners.len(), 1);
    let replacement = producer(&registry, &request);
    let current = registry.read(source.token);
    let current_source = current.source().unwrap();
    assert_ne!(source.mapping.buffer, current_source.mapping.buffer);
    assert!(
        current_source
            .mapping
            .clock()
            .producer_lease
            .load(Ordering::Acquire)
            > old_lease
    );
    assert_eq!(
        unsafe { mr_shared_audio_buffer_stats(current_source.mapping.buffer.as_ptr()) }.epoch,
        2
    );
    assert_eq!(registry.control().owners.len(), 2);
    drop(current);
    drop(read);
    collect(&registry);
    assert_eq!(registry.control().owners.len(), 1);
    drop(replacement);
    collect(&registry);
    assert!(registry.control().owners.is_empty());
}

#[test]
fn stale_producer_drop_cannot_close_a_replacement_grant() {
    let registry = registry();
    let request = request("one");
    let old = producer(&registry, &request);
    let current = producer(&registry, &request);
    let read = registry.read(token(b"one"));
    let source = read.source().unwrap();
    let lease = source
        .mapping
        .clock()
        .producer_lease
        .load(Ordering::Acquire);
    drop(old);
    assert_eq!(
        source
            .mapping
            .clock()
            .producer_lease
            .load(Ordering::Acquire),
        lease
    );
    collect(&registry);
    assert_eq!(registry.control().owners.len(), 1);
    drop(current);
    collect(&registry);
    assert_eq!(registry.control().owners.len(), 1);
    drop(read);
    collect(&registry);
    assert!(registry.control().owners.is_empty());
}

#[test]
fn invalidated_epoch_does_not_keep_a_closed_source_slot() {
    let registry = registry();
    let owner = producer(&registry, &request("invalidated"));
    {
        let read = registry.read(token(b"invalidated"));
        let source = read.source().unwrap();
        unsafe { mr_shared_audio_buffer_request_epoch(source.mapping.buffer.as_ptr()) };
        assert_eq!(
            unsafe { mr_shared_audio_buffer_stats(source.mapping.buffer.as_ptr()) }.epoch,
            0
        );
        drop(owner);
        assert_eq!(
            source
                .mapping
                .clock()
                .producer_lease
                .load(Ordering::Acquire),
            0
        );
    }
    collect(&registry);
    assert!(registry.control().owners.is_empty());
    assert!(registry.read(token(b"invalidated")).source().is_none());
}

#[test]
fn closed_source_retirement_pressure_stays_bounded_until_readers_drain() {
    let registry = registry();
    let request = request("pinned");
    let mut readers = Vec::new();
    for _ in 0..MAX_RETAINED {
        let owner = producer(&registry, &request);
        readers.push(registry.read(token(b"pinned")));
        drop(owner);
        collect(&registry);
    }
    assert_eq!(registry.control().owners.len(), MAX_RETAINED);
    assert_eq!(
        registry.attach(&request).err().unwrap().kind(),
        io::ErrorKind::WouldBlock
    );
    drop(readers);
    collect(&registry);
    assert!(registry.control().owners.is_empty());
    drop(producer(&registry, &request));
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct DescriptorLimit {
    current: u64,
    maximum: u64,
}

unsafe extern "C" {
    fn getrlimit(resource: i32, limit: *mut DescriptorLimit) -> i32;
    fn setrlimit(resource: i32, limit: *const DescriptorLimit) -> i32;
    fn mprotect(address: *mut std::ffi::c_void, length: usize, protection: i32) -> i32;
    fn fcntl(descriptor: i32, command: i32, ...) -> i32;
}

#[test]
fn grant_descriptor_failure_and_registry_reclamation_in_an_isolated_process() {
    const CHILD: &str = "MIXRELAY_SOURCE_REGISTRY_RETIREMENT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "source_registry::tests::registration::grant_descriptor_failure_and_registry_reclamation_in_an_isolated_process", "--test-threads=1"])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    exercise_failed_grant_and_reclamation();
}

fn exercise_failed_grant_and_reclamation() {
    use std::os::fd::AsRawFd;

    let registry = registry();
    let owner = producer(&registry, &request("closing"));
    let read = registry.read(token(b"closing"));
    let source = read.source().unwrap();
    let address = source.mapping.buffer.as_ptr().cast();
    let descriptor = source.mapping.descriptor.as_raw_fd();
    let clock = source.mapping.clock();
    let lease = clock.producer_lease.load(Ordering::Acquire);
    let epoch = unsafe { mr_shared_audio_buffer_stats(source.mapping.buffer.as_ptr()) }.epoch;
    let grant_claim = source.claim_producer().unwrap();
    // Match attach's grant-before-publication interval. Drop must signal closure
    // even while that remote control operation holds the producer guard.
    drop(owner);
    assert_eq!(clock.producer_lease.load(Ordering::Acquire), 0);
    let mut limit = DescriptorLimit::default();
    assert_eq!(unsafe { getrlimit(8, &raw mut limit) }, 0);
    let exhausted = DescriptorLimit {
        current: limit.current.min(64),
        ..limit
    };
    let mut filled = Vec::with_capacity(64);
    assert_eq!(unsafe { setrlimit(8, &raw const exhausted) }, 0);
    // F_DUPFD_CLOEXEC needs a valid descriptor minimum below the limit. Fill
    // that small range with owned duplicates; never close inherited descriptors.
    let fill_error = (0..64).find_map(|_| match source.mapping.descriptor.try_clone() {
        Ok(descriptor) => {
            filled.push(descriptor);
            None
        }
        Err(error) => Some(error),
    });
    let failed = source.grant(lease + 1, None);
    // Restore before any assertions or further descriptor allocations. The real
    // try_clone failed without changing the stream, not a mocked grant result.
    assert_eq!(unsafe { setrlimit(8, &raw const limit) }, 0);
    drop(filled);
    assert_eq!(fill_error.unwrap().raw_os_error(), Some(24));
    assert_eq!(failed.err().unwrap().raw_os_error(), Some(24));
    assert_eq!(
        unsafe { mr_shared_audio_buffer_stats(source.mapping.buffer.as_ptr()) }.epoch,
        epoch
    );
    assert_eq!(clock.producer_lease.load(Ordering::Acquire), 0);
    drop(grant_claim);
    collect(&registry);
    assert_eq!(registry.control().owners.len(), 1);
    assert_eq!(unsafe { mprotect(address, 1, 3) }, 0);
    assert_ne!(unsafe { fcntl(descriptor, 1) }, -1);
    drop(read);
    collect(&registry);
    assert!(registry.control().owners.is_empty());
    assert_eq!(unsafe { mprotect(address, 1, 3) }, -1);
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(12));
    assert_eq!(unsafe { fcntl(descriptor, 1) }, -1);
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(9));
}
