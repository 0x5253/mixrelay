//! Native lifecycle parity uses the production Rust runtime and actual shared
//! mappings, not the removed singleton driver's globals or fixture aliases.
use mixrelay_macos_transport::shared_audio_buffer::{
    mr_shared_audio_buffer_epoch_pending, mr_shared_audio_buffer_producer_activity,
};

use super::{IoOperation, OK, Transport, native_fixture, start};
use crate::abi::Timestamp;
use mixrelay_macos_transport::client_pcm::ClientPcmReader;

#[test]
fn new_engine_inventory_replaces_mapping_with_lower_or_equal_generation() {
    for generation in [2, 90] {
        let fixture = native_fixture();
        let old = Transport::new(10, 2, 2);
        let first = super::Message::snapshot(&[(&old, "Device", 48_000, 90)], 90);
        assert!(first.apply(&fixture));
        let id = fixture.registry.devices()[0];
        start(&fixture, id);
        assert_eq!(fixture.registry.set_rate(id, 96_000), OK);
        let (_, stale_request) = fixture.pending();
        let new = Transport::new(10, 2, 2);
        let next = super::Message::snapshot(&[(&new, "Device", 48_000, generation)], generation);
        super::connect(&fixture, 2);
        assert!(next.apply_from(&fixture, 2));
        let (_, replacement_request) = fixture.pending();
        assert!(replacement_request > stale_request);
        // Delayed CoreAudio replies must not consume the new owned attachment.
        assert_eq!(
            fixture
                .registry
                .perform(id, crate::abi::CONFIG_ACTION, stale_request),
            OK
        );
        assert_eq!(
            fixture
                .registry
                .abort(id, crate::abi::CONFIG_ACTION, stale_request),
            OK
        );
        assert_eq!(fixture.perform(), OK);
        assert_eq!(fixture.registry.devices(), vec![id]);
        assert_eq!(fixture.registry.snapshot(id).unwrap().sample_rate, 96_000);
        assert_eq!(
            (
                old.clock().running,
                new.clock().running,
                new.clock().sample_rate
            ),
            (0, 1, 96_000)
        );
        pcm(&fixture, id, true, 0, 0.75);
        assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.75_f32.to_bits(); 64]);
        assert!(!first.apply_from(&fixture, 1));
        assert_eq!(fixture.registry.snapshot(id).unwrap().sample_rate, 96_000);
    }
}

#[test]
fn new_connection_fences_old_messages_before_first_valid_inventory() {
    let fixture = native_fixture();
    let old = Transport::new(10, 2, 2);
    assert!(super::Message::snapshot(&[(&old, "Device", 48_000, 90)], 90).apply(&fixture));
    let id = fixture.registry.devices()[0];
    super::connect(&fixture, 2);
    // A malformed new peer cannot erase devices or reopen the old authority.
    assert!(!super::Message::snapshot(&[(&old, "Device", 12345, 1)], 1).apply_from(&fixture, 2));
    assert!(!super::Message::snapshot(&[], 91).apply_from(&fixture, 1));
    assert_eq!(fixture.registry.devices(), vec![id]);
    // Preserve old identities until a fully validated new list removes them.
    assert!(super::Message::snapshot(&[], 1).apply_from(&fixture, 2));
    assert!(fixture.registry.devices().is_empty());
}

#[test]
fn removal_requires_inventory_from_current_connection() {
    use mixrelay_macos_transport::control_inventory::mr_hal_control_create_endpoint_remove;

    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 2);
    let inventory = super::Message::snapshot(&[(&transport, "Device", 48_000, 1)], 1);
    assert!(inventory.apply(&fixture));
    let id = fixture.registry.devices()[0];
    let removal = super::Message(unsafe {
        mr_hal_control_create_endpoint_remove(transport.endpoint.as_ptr(), 2)
    });
    assert!(!removal.0.is_null());
    super::connect(&fixture, 2);
    assert!(!removal.apply_from(&fixture, 1));
    assert!(!removal.apply_from(&fixture, 2));
    assert_eq!(fixture.registry.devices(), vec![id]);
    assert!(inventory.apply_from(&fixture, 2));
    assert!(removal.apply_from(&fixture, 2));
    assert!(fixture.registry.devices().is_empty());
}

#[test]
fn same_mapping_on_new_connection_does_not_reset_running_stream() {
    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 2);
    let message = super::Message::snapshot(&[(&transport, "Device", 48_000, 1)], 1);
    assert!(message.apply(&fixture));
    let id = fixture.registry.devices()[0];
    start(&fixture, id);
    pcm(&fixture, id, true, 0, 0.25);
    let epoch = transport.clock().epoch;
    super::connect(&fixture, 2);
    assert!(message.apply_from(&fixture, 2));
    assert_eq!(transport.clock().epoch, epoch);
    assert!(crate::device::lock(&fixture.host.requests).is_empty());
    assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.25_f32.to_bits(); 64]);
}

fn pcm(fixture: &super::Fixture, device: u32, render: bool, frame: u64, value: f32) -> [u32; 64] {
    let mut samples = [value; 64];
    assert!(unsafe {
        fixture.registry.io(IoOperation {
            client: 42,
            device,
            stream: device + if render { 2 } else { 1 },
            render,
            frame,
            samples: samples.as_mut_ptr().cast(),
            frames: 32,
        })
    });
    samples.map(f32::to_bits)
}

#[test]
fn registered_output_client_can_supply_pcm_while_another_client_holds_device_io() {
    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 2);
    let id = transport.add(&fixture, "Separate IO and output clients", 48_000);
    assert_eq!(fixture.registry.client(id, 43, true), OK);
    assert_eq!(fixture.registry.start(id, 43, 1), OK);
    // A new output client may arrive after the device has already started.
    assert_eq!(fixture.registry.client(id, 42, true), OK);
    pcm(&fixture, id, true, 0, 0.25);
    assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.25_f32.to_bits(); 64]);
    // A balanced Start/Stop on the output client cannot erase committed PCM
    // while another client still holds the device's IO lifetime.
    assert_eq!(fixture.registry.start(id, 42, 2), OK);
    assert_eq!(fixture.registry.stop(id, 42), OK);
    assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.25_f32.to_bits(); 64]);
    assert_eq!(fixture.registry.client(id, 42, false), OK);
    assert_eq!(pcm(&fixture, id, false, 0, 0.9), [0; 64]);
    assert_eq!(fixture.registry.client(id, 42, true), OK);
    pcm(&fixture, id, true, 32, 0.5);
    assert_eq!(pcm(&fixture, id, false, 32, 0.0), [0.5_f32.to_bits(); 64]);
    assert_eq!(fixture.registry.stop(id, 43), OK);
    assert_eq!(pcm(&fixture, id, false, 32, 0.9), [0; 64]);
    assert_eq!(fixture.registry.start(id, 43, 3), OK);
    // The first device start opens a new timeline for every registered queue.
    assert_eq!(pcm(&fixture, id, false, 32, 0.9), [0; 64]);
    pcm(&fixture, id, true, 0, 0.75);
    assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.75_f32.to_bits(); 64]);
}

#[test]
fn callback_evidence_distinguishes_missing_capture_from_published_digital_silence() {
    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 2);
    let id = transport.add(&fixture, "Capture evidence", 48_000);
    start(&fixture, id);
    let device = fixture.registry.pin(id).unwrap();
    device.io_trace.set_enabled(true);
    pcm(&fixture, id, true, 0, 0.25);
    assert_eq!(
        pcm(&fixture, id, false, 16, 0.9),
        std::array::from_fn(|index| if index < 32 { 0.25_f32.to_bits() } else { 0 })
    );
    pcm(&fixture, id, true, 64, 0.0);
    assert_eq!(pcm(&fixture, id, false, 64, 0.9), [0; 64]);
    let bytes = device.io_trace.snapshot();
    let words: Vec<_> = bytes
        .chunks_exact(8)
        .map(|value| u64::from_ne_bytes(value.try_into().unwrap()))
        .collect();
    let rows: Vec<_> = words[crate::io_trace::HEADER_WORDS..]
        .chunks_exact(11)
        .filter(|row| row[0] != 0)
        .collect();
    assert_eq!(rows.len(), 4);
    assert_eq!(&rows[1][1..7], &[0, 16, 32, 1, 16, 16]);
    assert_eq!(&rows[3][1..7], &[0, 64, 32, 1, 32, 32]);
}

#[test]
fn native_multi_client_activity_and_restart_discard_previous_capture() {
    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 2);
    let id = transport.add(&fixture, "Clients", 48_000);
    let render = unsafe { super::mr_endpoint_transport_render_buffer(transport.pointer) };
    start(&fixture, id);
    assert_eq!(
        unsafe { mr_shared_audio_buffer_producer_activity(render) },
        2
    );
    assert_eq!(fixture.registry.client(id, 43, true), OK);
    assert_eq!(fixture.registry.start(id, 43, 2), OK);
    pcm(&fixture, id, true, 0, 0.5);
    assert_eq!(
        unsafe { mr_shared_audio_buffer_producer_activity(render) },
        3
    );
    assert_eq!(fixture.registry.stop(id, 42), OK);
    assert_eq!(
        (transport.clock().running, unsafe {
            mr_shared_audio_buffer_producer_activity(render)
        }),
        (1, 0)
    );
    let mut next = [0.75_f32; 64];
    assert!(unsafe {
        fixture.registry.io(IoOperation {
            device: id,
            stream: id + 2,
            render: true,
            client: 43,
            frame: 32,
            samples: next.as_mut_ptr().cast(),
            frames: 32,
        })
    });
    assert_eq!(pcm(&fixture, id, false, 32, 0.0), [0.75_f32.to_bits(); 64]);
    assert_eq!(fixture.registry.stop(id, 43), OK);
    assert_eq!(transport.clock().running, 0);
    assert_eq!(
        unsafe { mr_shared_audio_buffer_producer_activity(render) },
        1
    );
    assert_eq!(pcm(&fixture, id, false, 0, 0.9), [0; 64]);
    assert_eq!(fixture.registry.start(id, 42, 3), OK);
    assert_eq!(pcm(&fixture, id, false, 0, 0.9), [0; 64]);
    pcm(&fixture, id, true, 0, 0.25);
    assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.25_f32.to_bits(); 64]);
}

#[test]
fn rejected_native_start_keeps_shared_clients_stopped_until_mapping_replacement() {
    use mixrelay_macos_transport::{
        client_pcm::ClientPcmSnapshot,
        shared_audio_buffer::{
            mr_shared_audio_buffer_requires_replacement, mr_shared_audio_buffer_try_begin_epoch,
        },
    };

    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 2);
    let id = transport.add(&fixture, "Exhausted native start", 48_000);
    assert_eq!(fixture.registry.client(id, 42, true), OK);
    let (_, snapshot) = crate::native::client_snapshot(&fixture.registry, None).unwrap();
    // SAFETY: this test owns the complete snapshot and its retained immutable grants.
    let readers = unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }.unwrap();
    // SAFETY: the fixture owns this mapping and no IO producer has started.
    let render = unsafe { super::mr_endpoint_transport_render_buffer(transport.pointer) };
    for _ in 0..=u16::MAX {
        // SAFETY: the test is the sole producer-lifecycle owner of this idle ring.
        if unsafe { mr_shared_audio_buffer_try_begin_epoch(render) } == 0 {
            break;
        }
    }
    // SAFETY: the live test mapping retains this exhausted ring.
    assert!(unsafe { mr_shared_audio_buffer_requires_replacement(render) });
    assert_eq!(
        fixture.registry.start(id, 42, 100),
        crate::abi::NATIVE_ERROR
    );
    assert!(!fixture.registry.snapshot(id).unwrap().running);
    assert!(
        readers
            .iter()
            .all(|reader| reader.clock().is_none_or(|clock| !clock.running))
    );
    assert!(
        !fixture
            .registry
            .timestamp(id, 100, &mut Timestamp::default())
    );
    assert_eq!(fixture.registry.replacement_endpoint(), Some([10; 16]));
    let replacement = Transport::new(10, 2, 2);
    assert_eq!(replacement.add(&fixture, "Replacement", 48_000), id);
    assert_eq!(fixture.perform(), OK);
    assert_eq!(fixture.registry.start(id, 42, 200), OK);
    pcm(&fixture, id, true, 0, 0.375);
    assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.375_f32.to_bits(); 64]);
}

#[test]
fn every_native_rate_commit_rearms_capture_before_callbacks_resume() {
    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 2);
    let id = transport.add(&fixture, "Rate epochs", 48_000);
    start(&fixture, id);
    let capture = unsafe { super::mr_endpoint_transport_capture_buffer(transport.pointer) };
    let mut previous_seed = 0;
    for &rate in crate::formats::SAMPLE_RATES {
        pcm(&fixture, id, true, 32, 0.75);
        assert_eq!(pcm(&fixture, id, false, 32, 0.0), [0.75_f32.to_bits(); 64]);
        assert_eq!(fixture.registry.set_rate(id, rate), OK);
        assert_eq!(fixture.perform(), OK);
        assert_eq!(
            (transport.clock().sample_rate, transport.clock().running),
            (rate, 1)
        );
        assert!(!unsafe { mr_shared_audio_buffer_epoch_pending(capture) });
        assert_eq!(pcm(&fixture, id, false, 0, 0.9), [0; 64]);
        pcm(&fixture, id, true, 0, 0.25);
        assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.25_f32.to_bits(); 64]);
        let mut timestamp = Timestamp::default();
        assert!(fixture.registry.timestamp(id, 0, &mut timestamp));
        assert!(timestamp.seed > previous_seed);
        previous_seed = timestamp.seed;
    }
}

#[test]
fn rate_only_perform_keeps_client_queue_grants_and_rejects_old_rate_pcm() {
    use mixrelay_macos_transport::client_pcm::ClientPcmSnapshot;

    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 2);
    let id = transport.add(&fixture, "Rate grants", 48_000);
    start(&fixture, id);
    pcm(&fixture, id, true, 0, 0.75);
    let (signature, snapshot) = crate::native::client_snapshot(&fixture.registry, None).unwrap();
    // SAFETY: this test owns the complete snapshot and its retained immutable grants.
    let readers = unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }.unwrap();
    let before = readers[0].clock().unwrap();
    assert_eq!(fixture.registry.set_rate(id, 96_000), OK);
    assert_eq!(fixture.perform(), OK);
    // A rate-only change keeps every queue and grant: the same mappings follow the new
    // clock and no new grant list is published.
    assert!(crate::native::client_snapshot(&fixture.registry, Some(&signature)).is_none());
    let after = readers[0].clock().unwrap();
    assert_eq!(
        (after.incarnation, after.rate, after.running, after.first),
        (before.incarnation, 96_000, true, None)
    );
    let mut output = [9.0; 64];
    let mut coverage = [true; 32];
    assert!(readers[0].read(after, 0, &mut output, &mut coverage));
    assert_eq!((output.map(f32::to_bits), coverage), ([0; 64], [false; 32]));
    assert_eq!(pcm(&fixture, id, false, 0, 0.9), [0; 64]);
    pcm(&fixture, id, true, 0, 0.25);
    assert!(readers[0].read(readers[0].clock().unwrap(), 0, &mut output, &mut coverage));
    assert_eq!(
        (output.map(f32::to_bits), coverage),
        ([0.25_f32.to_bits(); 64], [true; 32])
    );
}

/// Measures one Perform's frozen commit, with the fixture host's own allocations
/// excluded, then completes the Perform as the registry does: re-provisioning maps queues
/// on control after that step.
fn measured_perform(fixture: &super::Fixture) -> (i32, u64) {
    // Host notifications append to the fixture's log, reserved here; its reentrant
    // registry queries allocate on the host side of the boundary and are skipped.
    crate::device::lock(&fixture.host.changes).reserve(8);
    let registry = fixture
        .host
        .registry
        .swap(std::ptr::null_mut(), std::sync::atomic::Ordering::AcqRel);
    let (id, generation) = fixture.pending();
    let mut status = OK;
    let usage = allocation_counter::measure(|| {
        status = fixture.registry.commit(
            id,
            crate::abi::CONFIG_ACTION,
            generation,
            crate::device::Device::prepare_scratch,
        );
    });
    fixture
        .host
        .registry
        .store(registry, std::sync::atomic::Ordering::Release);
    if status == OK {
        fixture.registry.reprovision(id);
    }
    (status, usage.count_total)
}

/// Renders 32 four-channel frames of 0.5 as `client`, reporting whether a queue took them.
fn render(fixture: &super::Fixture, id: u32, client: u32) -> bool {
    let mut samples = [0.5_f32; 128];
    // SAFETY: the client owns 32 four-channel Float32 frames in the committed format.
    unsafe {
        fixture.registry.io(IoOperation {
            client,
            device: id,
            stream: id + 2,
            render: true,
            frame: 0,
            samples: samples.as_mut_ptr().cast(),
            frames: 32,
        })
    }
}

/// The clients and channel counts of the current grant list.
fn grants(fixture: &super::Fixture) -> Vec<(u32, u32)> {
    crate::native::client_snapshot(&fixture.registry, None).map_or_else(
        Vec::new,
        |(_, snapshot)| {
            // SAFETY: this test owns the complete snapshot and its retained immutable grants.
            unsafe {
                mixrelay_macos_transport::client_pcm::ClientPcmSnapshot::decode(snapshot.as_raw())
            }
            .unwrap()
            .iter()
            .map(|reader| (reader.client(), reader.channels()))
            .collect()
        },
    )
}

#[test]
fn perform_maps_and_allocates_nothing_even_when_queues_run_out_of_epochs() {
    use mixrelay_macos_transport::client_pcm::ClientPcmSnapshot;

    let fixture = native_fixture();
    let old = Transport::new(10, 2, 2);
    let id = old.add(&fixture, "Committed", 48_000);
    start(&fixture, id);
    // A first Perform initializes each lazily created OS lock and is not measured.
    assert_eq!(fixture.registry.set_rate(id, 88_200), OK);
    assert_eq!(fixture.perform(), OK);
    let wider = Transport::new(10, 4, 2);
    assert_eq!(wider.add(&fixture, "Committed", 88_200), id);
    assert_eq!(measured_perform(&fixture), (OK, 0));
    // Client 42 holds IO throughout: CoreAudio pauses it around Perform without StopIO or
    // StartIO, so each change must leave it a queue.
    assert!(render(&fixture, id, 42));
    let mut transports = vec![wider];
    let (mut signature, snapshot) =
        crate::native::client_snapshot(&fixture.registry, None).unwrap();
    // SAFETY: this test owns the complete snapshot and its retained immutable grants.
    let mut readers = unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }.unwrap();
    let mut rate = 88_200;
    let mut detached = false;
    for _ in 0..3 * usize::from(u16::MAX) {
        rate = if rate == 96_000 { 48_000 } else { 96_000 };
        assert_eq!(fixture.registry.set_rate(id, rate), OK);
        let (status, allocations) = measured_perform(&fixture);
        assert_eq!(allocations, 0, "rate commit allocated");
        if status != OK {
            // The endpoint rings ran out of epochs first; a fresh mapping of the same
            // shape keeps the client queue, and that commit maps nothing either.
            assert_eq!(status, crate::abi::NATIVE_ERROR);
            let fresh = Transport::new(10, 4, 2);
            let current = fixture.registry.snapshot(id).unwrap().sample_rate;
            assert_eq!(fresh.add(&fixture, "Committed", current), id);
            assert_eq!(measured_perform(&fixture), (OK, 0));
            transports.push(fresh);
            rate = current;
        }
        if let Some((next, snapshot)) =
            crate::native::client_snapshot(&fixture.registry, Some(&signature))
        {
            // SAFETY: as above.
            let next_readers = unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }.unwrap();
            if readers.iter().all(ClientPcmReader::producer_retired) {
                detached = true;
                readers = next_readers;
                break;
            }
            signature = next;
            readers = next_readers;
        }
    }
    assert!(detached, "no rate commit detached an exhausted queue");
    // The same Perform mapped a replacement, with no StartIO, and readers picked it up.
    assert_eq!(
        readers
            .iter()
            .map(|reader| (reader.client(), reader.producer_retired()))
            .collect::<Vec<_>>(),
        [(42, false)]
    );
    assert!(render(&fixture, id, 42));
    assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.5_f32.to_bits(); 64]);
}

#[test]
fn a_format_only_perform_with_healthy_running_clients_takes_no_extra_freeze() {
    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 2);
    let id = transport.add(&fixture, "Formats", 48_000);
    start(&fixture, id);
    let device = fixture.registry.pin(id).unwrap();
    let grants = crate::native::client_snapshot(&fixture.registry, None)
        .unwrap()
        .0;
    assert_eq!(
        fixture
            .registry
            .set_format(id + 2, 48_000, mixrelay_pcm_ring::SampleFormat::Signed24),
        OK
    );
    let freezes = device.freezes.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(fixture.perform(), OK);
    // Only the commit froze IO: client 42 keeps its queue, so re-provisioning maps,
    // installs and republishes nothing.
    assert_eq!(
        device.freezes.load(std::sync::atomic::Ordering::Relaxed),
        freezes + 1
    );
    assert!(crate::native::client_snapshot(&fixture.registry, Some(&grants)).is_none());
    drop(device);
    // A control tick with every running client provisioned freezes nothing either.
    let device = fixture.registry.pin(id).unwrap();
    let freezes = device.freezes.load(std::sync::atomic::Ordering::Relaxed);
    drop(device);
    fixture.registry.tick();
    let device = fixture.registry.pin(id).unwrap();
    assert_eq!(
        device.freezes.load(std::sync::atomic::Ordering::Relaxed),
        freezes
    );
}

#[test]
fn frozen_native_operations_neither_allocate_nor_free() {
    let fixture = native_fixture();
    let old = Transport::new(10, 2, 2);
    let id = old.add(&fixture, "Frees", 48_000);
    start(&fixture, id);
    // A first commit initializes each lazily created OS lock and is not measured.
    assert_eq!(fixture.registry.set_rate(id, 88_200), OK);
    assert_eq!(fixture.perform(), OK);
    let native = fixture.registry.native;
    let device = fixture.registry.pin(id).unwrap();
    let runtime = device.runtime.load(std::sync::atomic::Ordering::Acquire);
    let control = crate::device::lock(&device.control);
    // A new endpoint mapping and a client queue, each prepared on control.
    let wider = Transport::new(10, 4, 2);
    let fd = std::os::fd::AsRawFd::as_raw_fd(&wider.fd);
    let mut geometry = crate::abi::Geometry::default();
    // SAFETY: the transport owns its descriptor; the geometry is a live local.
    assert!(unsafe { (native.inspect)(fd, wider.size, &raw mut geometry) });
    // SAFETY: as above.
    let mapping = unsafe { (native.prepare)(&raw const geometry, fd, wider.size) };
    assert!(!mapping.is_null());
    let frozen = device.freeze();
    let usage = allocation_counter::measure(|| {
        // SAFETY: control and the freeze retain this live runtime and exclude IO.
        assert!(unsafe { (native.replace)(runtime, mapping, 88_200) });
    });
    drop(frozen);
    // SAFETY: control retains the runtime; the freeze has ended.
    unsafe { (native.release)(runtime) };
    // Each preparation box used to be freed inside the freeze.
    assert_eq!((usage.count_total, usage.count_current), (0, 0));
    // SAFETY: control retains the live runtime; mapping runs outside the freeze.
    let queue = unsafe { native.prepare_queue(runtime, 42) }
        .unwrap()
        .unwrap();
    let frozen = device.freeze();
    let usage = allocation_counter::measure(|| {
        // SAFETY: control and the freeze retain this live runtime and exclude IO.
        assert!(unsafe {
            native.update_client(
                runtime,
                42,
                crate::io_outcome::ClientChange::Install,
                Some(queue),
            )
        });
    });
    drop(frozen);
    // SAFETY: as above.
    unsafe { (native.release)(runtime) };
    assert_eq!((usage.count_total, usage.count_current), (0, 0));
    drop(control);
}

#[test]
fn a_refused_rate_change_leaves_queues_epochs_revision_and_clock_unchanged() {
    use mixrelay_macos_transport::{
        client_pcm::ClientPcmSnapshot, shared_audio_buffer::mr_shared_audio_buffer_try_begin_epoch,
    };

    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 2);
    let id = transport.add(&fixture, "Refused rate", 48_000);
    start(&fixture, id);
    pcm(&fixture, id, true, 0, 0.75);
    let (signature, snapshot) = crate::native::client_snapshot(&fixture.registry, None).unwrap();
    // SAFETY: this test owns the complete snapshot and its retained immutable grants.
    let readers = unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }.unwrap();
    let before = readers[0].clock().unwrap();
    // SAFETY: the fixture owns this mapping; no render callback runs during the loop.
    let render = unsafe { super::mr_endpoint_transport_render_buffer(transport.pointer) };
    for _ in 0..=u16::MAX {
        // SAFETY: as above; the test is the sole lifecycle owner of this idle ring.
        if unsafe { mr_shared_audio_buffer_try_begin_epoch(render) } == 0 {
            break;
        }
    }
    assert_eq!(fixture.registry.set_rate(id, 96_000), OK);
    assert_eq!(fixture.perform(), crate::abi::NATIVE_ERROR);
    // The endpoint ring was checked before any client queue or the shared clock changed.
    assert!(crate::native::client_snapshot(&fixture.registry, Some(&signature)).is_none());
    assert_eq!(readers[0].clock(), Some(before));
    assert_eq!(fixture.registry.snapshot(id).unwrap().sample_rate, 48_000);
    assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.75_f32.to_bits(); 64]);
}

#[test]
fn queues_a_frozen_change_retires_are_released_only_after_the_freeze() {
    use mixrelay_macos_transport::client_pcm::ClientPcmSnapshot;

    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 2);
    let id = transport.add(&fixture, "Released", 48_000);
    start(&fixture, id);
    assert_eq!(fixture.registry.client(id, 43, true), OK);
    let (_, snapshot) = crate::native::client_snapshot(&fixture.registry, None).unwrap();
    // SAFETY: this test owns the complete snapshot and its retained immutable grants.
    let readers = unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }.unwrap();
    let removed = readers.iter().find(|reader| reader.client() == 43).unwrap();
    let device = fixture.registry.pin(id).unwrap();
    let runtime = device.runtime.load(std::sync::atomic::Ordering::Acquire);
    let control = crate::device::lock(&device.control);
    let frozen = device.freeze();
    // SAFETY: control and the freeze retain this live runtime and exclude IO readers.
    assert!(unsafe {
        fixture.registry.native.update_client(
            runtime,
            43,
            crate::io_outcome::ClientChange::Remove,
            None,
        )
    });
    // Unmapping inside the freeze would stall every waiting IO callback.
    assert!(!removed.producer_retired());
    drop(frozen);
    // SAFETY: control retains the runtime; the freeze has ended.
    unsafe { (fixture.registry.native.release)(runtime) };
    assert!(removed.producer_retired());
    drop(control);
    drop(device);
    // The registry path releases after its own freeze as well.
    assert_eq!(fixture.registry.client(id, 44, true), OK);
    let (_, snapshot) = crate::native::client_snapshot(&fixture.registry, None).unwrap();
    // SAFETY: as above.
    let readers = unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }.unwrap();
    let joined = readers.iter().find(|reader| reader.client() == 44).unwrap();
    assert_eq!(fixture.registry.client(id, 44, false), OK);
    assert!(joined.producer_retired());
}

#[test]
fn a_geometry_change_reprovisions_clients_holding_io_without_start_io() {
    use mixrelay_macos_transport::client_pcm::ClientPcmSnapshot;

    let fixture = native_fixture();
    let old = Transport::new(10, 2, 2);
    let id = old.add(&fixture, "Before", 48_000);
    start(&fixture, id);
    // Client 43 joins the running device, so it may render before its own StartIO.
    assert_eq!(fixture.registry.client(id, 43, true), OK);
    let (_, snapshot) = crate::native::client_snapshot(&fixture.registry, None).unwrap();
    // SAFETY: this test owns the complete snapshot and its retained immutable grants.
    let before = unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }.unwrap();
    assert_eq!(before.len(), 2);
    let wider = Transport::new(10, 4, 2);
    assert_eq!(wider.add(&fixture, "After", 48_000), id);
    // coreaudiod pauses IO around Perform but calls neither StopIO nor StartIO.
    assert_eq!(fixture.perform(), OK);
    assert_eq!(fixture.registry.snapshot(id).unwrap().render_channels, 4);
    // Queues of the old shape were unmapped after the freeze; client 42, which holds
    // IO, already has a queue of the new shape and readers see its new grant.
    assert!(before.iter().all(ClientPcmReader::producer_retired));
    assert_eq!(grants(&fixture), [(42, 4)]);
    assert!(render(&fixture, id, 42));
    assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.5_f32.to_bits(); 64]);
    // A client that does not hold IO gets its queue only when it starts IO.
    assert!(!render(&fixture, id, 43));
    assert_eq!(fixture.registry.start(id, 43, 2), OK);
    assert_eq!(grants(&fixture), [(42, 4), (43, 4)]);
    assert!(render(&fixture, id, 43));
}

#[test]
fn idle_replacement_keeps_native_rate_and_does_not_emit_stale_pcm() {
    let fixture = native_fixture();
    let old = Transport::new(10, 2, 2);
    let id = old.add(&fixture, "Before", 96_000);
    start(&fixture, id);
    pcm(&fixture, id, true, 0, 0.75);
    assert_eq!(fixture.registry.stop(id, 42), OK);
    let replacement = Transport::new(10, 2, 2);
    assert_eq!(replacement.add(&fixture, "After", 48_000), id);
    assert_eq!(fixture.perform(), OK);
    assert_eq!(
        (replacement.clock().sample_rate, replacement.clock().running),
        (96_000, 0)
    );
    assert_eq!(old.clock().running, 0);
    assert_eq!(pcm(&fixture, id, false, 0, 0.9), [0; 64]);
    assert_eq!(fixture.registry.start(id, 42, 4), OK);
    assert_eq!(pcm(&fixture, id, false, 0, 0.9), [0; 64]);
    pcm(&fixture, id, true, 0, 0.25);
    assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.25_f32.to_bits(); 64]);
}

#[test]
fn failed_native_attachment_preserves_running_mapping_clock_and_pcm() {
    let fixture = native_fixture();
    let old = Transport::new(10, 2, 2);
    let id = old.add(&fixture, "Still active", 96_000);
    start(&fixture, id);
    pcm(&fixture, id, true, 0, 0.75);
    let before = old.clock();
    let candidate = Transport::new(10, 7, 3);
    assert_eq!(
        candidate.add(&fixture, "Invalidated before commit", 48_000),
        id
    );
    // The descriptor was valid when queued. Invalidate only this test-owned
    // candidate before the real native prepare path; the active mapping is
    // separate and must remain untouched when attach rejects the candidate.
    unsafe { candidate.memory.cast::<u8>().write(0) };
    assert_eq!(fixture.perform(), crate::abi::NATIVE_ERROR);
    let after = old.clock();
    assert_eq!(
        (after.epoch, after.sample_rate, after.running),
        (before.epoch, 96_000, 1)
    );
    assert_eq!(fixture.registry.snapshot(id).unwrap().render_channels, 2);
    assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.75_f32.to_bits(); 64]);
    pcm(&fixture, id, true, 32, 0.25);
    assert_eq!(pcm(&fixture, id, false, 32, 0.0), [0.25_f32.to_bits(); 64]);
    assert_eq!(
        old.capture::<64>(&fixture, id, 32).map(f32::to_bits),
        [0.75_f32.to_bits(); 64]
    );
}

#[test]
fn native_runtime_mapping_outlives_caller_descriptor_and_mapping_view() {
    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 2);
    let id = transport.add(&fixture, "Owned mapping", 96_000);
    start(&fixture, id);
    pcm(&fixture, id, true, 0, 0.25);
    drop(transport);
    assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.25_f32.to_bits(); 64]);
    pcm(&fixture, id, true, 32, 0.5);
    assert_eq!(pcm(&fixture, id, false, 32, 0.0), [0.5_f32.to_bits(); 64]);
    let mut timestamp = Timestamp::default();
    assert!(fixture.registry.timestamp(id, 100, &mut timestamp));
    assert_eq!(fixture.registry.snapshot(id).unwrap().sample_rate, 96_000);
    assert_eq!(fixture.registry.stop(id, 42), OK);
    assert_eq!(fixture.registry.client(id, 42, false), OK);
    assert_eq!(fixture.registry.remove([10; 16]), OK);
    assert!(fixture.registry.snapshot(id).is_none());
}

#[test]
fn native_removal_releases_running_mapping_while_host_client_retains_device() {
    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 2);
    let id = transport.add(&fixture, "Removed while open", 48_000);
    start(&fixture, id);
    pcm(&fixture, id, true, 0, 0.75);
    assert_eq!(transport.clock().running, 1);
    assert_eq!(fixture.registry.remove([10; 16]), OK);
    assert_eq!(transport.clock().running, 0);
    let device = fixture.registry.pin(id).unwrap();
    assert!(
        device
            .runtime
            .load(std::sync::atomic::Ordering::Acquire)
            .is_null()
    );
    drop(device);
    let retired = fixture.registry.snapshot(id).unwrap();
    assert!(!retired.alive);
    assert!(!retired.running);
    assert_eq!(retired.client_count, 1);
    assert_eq!(pcm(&fixture, id, false, 0, 1.0), [0; 64]);
    let mut timestamp = Timestamp::default();
    assert!(!fixture.registry.timestamp(id, 100, &mut timestamp));
    assert_eq!(fixture.registry.stop(id, 42), OK);
    assert_eq!(fixture.registry.client(id, 42, false), OK);
    assert!(fixture.registry.snapshot(id).is_none());
    let replacement = transport.add(&fixture, "Fresh device", 48_000);
    assert_ne!(replacement, id);
    start(&fixture, replacement);
    pcm(&fixture, replacement, true, 0, 0.25);
    assert_eq!(
        pcm(&fixture, replacement, false, 0, 0.0),
        [0.25_f32.to_bits(); 64]
    );
}

/// Frame-indexed stereo words for the 32-frame block at `frame`. Words below 2^13 are
/// exact binary fractions in Float32, distinct per frame and lane.
fn loopback_words(frame: u64) -> [f32; 64] {
    std::array::from_fn(|index| {
        let position = frame + index as u64 / 2;
        let word = f32::from(u16::try_from(position % 8192).unwrap()) / 8192.0;
        if index % 2 == 0 { word } else { -word }
    })
}

/// Device IO through the shared audio return path with capture one block behind render, so one
/// rendered block is always buffered unread between cycles. Lost, repeated or reset PCM
/// changes the captured words.
struct Loopback<'a> {
    fixture: &'a super::Fixture,
    device: u32,
    /// Frame of the rendered block that has not been captured yet.
    buffered: u64,
}

impl<'a> Loopback<'a> {
    fn start(fixture: &'a super::Fixture, device: u32) -> Self {
        let loopback = Self {
            fixture,
            device,
            buffered: 0,
        };
        loopback.io(true, 0, &mut loopback_words(0));
        loopback
    }

    /// One IO cycle: verify the buffered block, then render the next one.
    fn cycle(&mut self) {
        let mut captured = [9.0_f32; 64];
        self.io(false, self.buffered, &mut captured);
        assert_eq!(
            captured.map(f32::to_bits),
            loopback_words(self.buffered).map(f32::to_bits),
            "loopback discontinuity at frame {}",
            self.buffered
        );
        self.buffered += 32;
        self.io(true, self.buffered, &mut loopback_words(self.buffered));
    }

    fn io(&self, render: bool, frame: u64, samples: &mut [f32; 64]) {
        // SAFETY: the fixture owns the registry; each block is 32 stereo Float32 frames.
        assert!(unsafe {
            self.fixture.registry.io(IoOperation {
                client: 42,
                device: self.device,
                stream: self.device + if render { 2 } else { 1 },
                render,
                frame,
                samples: samples.as_mut_ptr().cast(),
                frames: 32,
            })
        });
    }
}

#[test]
fn loopback_stays_continuous_while_other_clients_attach_run_and_detach() {
    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 2);
    let id = transport.add(&fixture, "Shared", 48_000);
    start(&fixture, id);
    let epoch = transport.clock().epoch;
    let mut loopback = Loopback::start(&fixture, id);
    let mut cycles = |count: u32| {
        for _ in 0..count {
            loopback.cycle();
        }
    };
    // Every transition below happens while one rendered block is still unread; the next
    // cycle captures that block before rendering again.
    cycles(16);
    // A second client attaches and starts IO on the running device.
    assert_eq!(fixture.registry.client(id, 43, true), OK);
    assert_eq!(fixture.registry.start(id, 43, 2), OK);
    cycles(16);
    // It stops, then detaches, each while the continuing stream has a block unread.
    assert_eq!(fixture.registry.stop(id, 43), OK);
    cycles(1);
    assert_eq!(fixture.registry.client(id, 43, false), OK);
    cycles(16);
    // A client whose process disappears without StopIO releases only its own claim.
    assert_eq!(fixture.registry.client(id, 44, true), OK);
    assert_eq!(fixture.registry.start(id, 44, 3), OK);
    cycles(16);
    assert_eq!(fixture.registry.client(id, 44, false), OK);
    cycles(16);
    let clock = transport.clock();
    assert_eq!(
        (clock.running, clock.epoch),
        (1, epoch),
        "the first client's stream was restarted"
    );
}

#[test]
fn pcm_inventory_includes_geometry_before_any_client_grant_exists() {
    use mixrelay_macos_transport::client_pcm::{ClientPcmSnapshot, EndpointGeometry};
    let fixture = native_fixture();
    let transport = Transport::new(33, 2, 4);
    transport.add(&fixture, "Idle capture", 48_000);
    let (_, snapshot) = crate::native::client_snapshot(&fixture.registry, None).unwrap();
    // SAFETY: the owned snapshot retains its complete geometry and grants.
    assert!(
        unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }
            .unwrap()
            .is_empty()
    );
    // SAFETY: the same retained snapshot supplies its checked geometry array.
    assert_eq!(
        unsafe { ClientPcmSnapshot::endpoints(snapshot.as_raw()) }.unwrap(),
        vec![EndpointGeometry {
            id: transport.endpoint,
            render_channels: 2,
            capture_channels: 4,
        }]
    );
}

#[test]
fn start_io_and_running_client_add_succeed_without_an_engine_connection() {
    let fixture = native_fixture();
    let transport = Transport::new(34, 2, 2);
    let id = transport.add(&fixture, "Independent IO", 192_000);
    // This native registry has no engine peer or source preparation callback.
    start(&fixture, id);
    assert_eq!(fixture.registry.client(id, 43, true), OK);
    assert_eq!(fixture.registry.start(id, 43, 1), OK);
    pcm(&fixture, id, true, 0, 0.625);
    assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.625_f32.to_bits(); 64]);
    assert_eq!(grants(&fixture).len(), 2);
}

#[test]
fn start_io_succeeds_with_an_unanswered_engine_publication() {
    let fixture = native_fixture();
    let transport = Transport::new(35, 2, 2);
    let id = transport.add(&fixture, "Unanswered publication", 48_000);
    // Retain the publication without delivering it or producing any engine reply.
    let (_signature, _publication) =
        crate::native::client_snapshot(&fixture.registry, None).unwrap();
    assert_eq!(fixture.registry.client(id, 42, true), OK);
    assert_eq!(fixture.registry.start(id, 42, 100), OK);
    let mut timestamp = Timestamp::default();
    assert!(fixture.registry.timestamp(id, 100, &mut timestamp));
    assert_eq!(timestamp.host_time, 100);
    assert_eq!(timestamp.sample_time, 0);
    pcm(&fixture, id, true, 0, 0.5);
    assert_eq!(pcm(&fixture, id, false, 0, 0.0), [0.5_f32.to_bits(); 64]);
}
