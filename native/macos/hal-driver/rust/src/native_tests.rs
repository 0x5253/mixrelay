//! No installed device, coreaudiod restart or filesystem fixture is involved:
//! these tests exercise the actual exported HAL runtime on anonymous mappings.
use std::ffi::c_void;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::Ordering;

use mixrelay_macos_transport::control_inventory::{
    InventoryEntry, mr_hal_control_create_inventory_snapshot,
};
use mixrelay_macos_transport::endpoint_transport::{
    EndpointClockSnapshot, EndpointTransport, EndpointTransportConfig,
    mr_endpoint_transport_capture_buffer, mr_endpoint_transport_clock_snapshot,
    mr_endpoint_transport_initialize, mr_endpoint_transport_render_buffer,
    mr_endpoint_transport_required_bytes,
};

use super::{Fixture, changed, request};
use crate::abi::{HostOps, OK};
use crate::registry::IoOperation;

#[path = "native_host_io_tests.rs"]
mod host_io;
#[path = "native_lifecycle_tests.rs"]
mod lifecycle;

unsafe extern "C" {
    fn mmap(
        address: *mut c_void,
        size: usize,
        protection: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, size: usize) -> i32;
    fn xpc_release(message: *mut c_void);
}

struct Message(*mut c_void);
impl Message {
    fn snapshot(entries: &[(&Transport, &str, u32, u64)], generation: u64) -> Self {
        let entries: Vec<_> = entries
            .iter()
            .map(|(transport, name, rate, generation)| {
                let mut entry = InventoryEntry {
                    endpoint_id: transport.endpoint,
                    sample_rate: *rate,
                    descriptor: transport.fd.as_raw_fd(),
                    mapped_size: transport.size,
                    generation: *generation,
                    ..InventoryEntry::default()
                };
                for (output, byte) in entry.display_name.iter_mut().zip(name.bytes()) {
                    *output = std::ffi::c_char::from_ne_bytes([byte]);
                }
                entry
            })
            .collect();
        let message = unsafe {
            mr_hal_control_create_inventory_snapshot(entries.as_ptr(), entries.len(), generation)
        };
        assert!(!message.is_null());
        Self(message)
    }

    fn apply(&self, fixture: &Fixture) -> bool {
        connect(fixture, 1);
        self.apply_from(fixture, 1)
    }

    fn apply_from(&self, fixture: &Fixture, authority: u64) -> bool {
        unsafe {
            crate::inventory::mr_hal_core_control_message(
                (&raw const *fixture.registry).cast_mut(),
                authority,
                self.0,
            )
        }
    }
}

fn connect(fixture: &Fixture, authority: u64) {
    assert!(unsafe {
        crate::inventory::mr_hal_core_control_connected(
            (&raw const *fixture.registry).cast_mut(),
            authority,
        )
    });
}
impl Drop for Message {
    fn drop(&mut self) {
        unsafe { xpc_release(self.0) };
    }
}

pub(super) struct Transport {
    pub(super) fd: OwnedFd,
    pointer: *mut EndpointTransport,
    memory: *mut c_void,
    pub(super) size: usize,
    endpoint: [u8; 16],
}
impl Transport {
    pub(super) fn new(endpoint: u8, render: u32, capture: u32) -> Self {
        Self::with_capacity(endpoint, render, capture, 256)
    }

    pub(super) fn with_capacity(endpoint: u8, render: u32, capture: u32, capacity: u32) -> Self {
        let config = EndpointTransportConfig {
            endpoint_id: [endpoint; 16],
            capacity_frames: capacity,
            render_channels: render,
            capture_channels: capture,
        };
        let size = unsafe { mr_endpoint_transport_required_bytes(&raw const config) };
        assert!(size > 0);
        let fd = mixrelay_macos_transport::create_shared_memory(size, true).unwrap();
        let memory = unsafe { mmap(std::ptr::null_mut(), size, 3, 1, fd.as_raw_fd(), 0) };
        assert_ne!(memory as isize, -1);
        let pointer = unsafe { mr_endpoint_transport_initialize(memory, size, &raw const config) };
        assert!(!pointer.is_null());
        Self {
            fd,
            pointer,
            memory,
            size,
            endpoint: config.endpoint_id,
        }
    }

    fn add(&self, fixture: &Fixture, name: &str, rate: u32) -> u32 {
        fixture
            .registry
            .upsert(
                self.endpoint,
                name,
                rate,
                (self.fd.try_clone().unwrap(), self.size),
            )
            .unwrap()
    }

    fn clock(&self) -> EndpointClockSnapshot {
        let mut clock = EndpointClockSnapshot::default();
        assert!(unsafe { mr_endpoint_transport_clock_snapshot(self.pointer, &raw mut clock) });
        clock
    }

    fn capture<const N: usize>(&self, fixture: &Fixture, id: u32, frames: u32) -> [f32; N] {
        let snapshot = fixture.registry.snapshot(id).unwrap();
        assert_eq!(fixture.registry.pin(id).unwrap().endpoint, self.endpoint);
        assert_eq!(N, frames as usize * snapshot.capture_channels as usize);
        let mut samples = [0.0; N];
        assert!(unsafe {
            fixture.registry.io(IoOperation {
                device: id,
                stream: id + 1,
                render: false,
                client: 42,
                frame: 0,
                samples: samples.as_mut_ptr().cast(),
                frames,
            })
        });
        samples
    }
}
impl Drop for Transport {
    fn drop(&mut self) {
        assert_eq!(unsafe { munmap(self.memory, self.size) }, 0);
    }
}

fn native_fixture() -> Fixture {
    let mut fixture = Fixture::new();
    let host = HostOps {
        context: (&raw const *fixture.host).cast_mut().cast(),
        changed,
        request_configuration: request,
    };
    // The installed driver host cannot inspect other processes.
    let registry = Box::into_raw(Box::new(crate::native::native_registry(
        host,
        crate::transport::ProcessInspection::SANDBOXED,
    )));
    fixture.registry = unsafe { Box::from_raw(registry) };
    fixture.host.registry.store(registry, Ordering::Release);
    fixture
}

#[test]
fn native_integer_capture_encodes_canonical_client_mix_gain_and_exact_mute() {
    let fixture = native_fixture();
    for (index, encoding) in crate::properties::NATIVE_FORMATS.into_iter().enumerate() {
        for channels in [1, 2, 7, 64] {
            let transport = Transport::new(
                u8::try_from(index * 4 + channels as usize % 4 + 1).unwrap(),
                channels,
                channels,
            );
            let id = transport.add(&fixture, "Native encoding", 48_000);
            assert_eq!(fixture.registry.set_format(id + 1, 48_000, encoding), OK);
            assert_eq!(fixture.registry.set_format(id + 2, 48_000, encoding), OK);
            if encoding != mixrelay_pcm_ring::SampleFormat::Float32 {
                assert_eq!(fixture.perform(), OK);
            }
            assert_eq!(fixture.registry.client(id, 1, true), OK);
            assert_eq!(fixture.registry.start(id, 1, 100), OK);
            let signal: Vec<_> = (0..32 * channels)
                .map(|sample| if sample % 2 == 0 { 0.25 } else { -0.125 })
                .collect();
            assert_native_render(&fixture, id, &transport, encoding, &signal);
            assert_native_capture(&fixture, id, &transport, encoding, &signal);
            assert_eq!(fixture.registry.stop(id, 1), OK);
            assert_eq!(fixture.registry.client(id, 1, false), OK);
            assert_eq!(fixture.registry.remove(transport.endpoint), OK);
            fixture.registry.tick();
        }
    }
}

fn native_block_io(fixture: &Fixture, id: u32, operation: (bool, u64), bytes: &mut [u8]) {
    let (render, frame) = operation;
    // SAFETY: fixture owns the registry and supplies 32 frames in the committed native
    // format. Byte storage is exclusive; Float32 callers provide aligned Vec storage.
    assert!(unsafe {
        crate::abi::mr_hal_core_io(
            (&raw const *fixture.registry).cast_mut(),
            id,
            id + if render { 2 } else { 1 },
            render,
            42,
            frame,
            bytes.as_mut_ptr().cast(),
            32,
        )
    });
}

fn assert_native_render(
    fixture: &Fixture,
    id: u32,
    _transport: &Transport,
    encoding: mixrelay_pcm_ring::SampleFormat,
    signal: &[f32],
) {
    let mut canonical = signal.to_vec();
    let mut storage = vec![0.0_f32; signal.len()];
    // SAFETY: the initialized Float32 allocation owns the full native input byte extent.
    let bytes = unsafe {
        std::slice::from_raw_parts_mut(
            storage.as_mut_ptr().cast::<u8>(),
            signal.len() * encoding.bytes_per_sample(),
        )
    };
    assert_eq!(fixture.registry.set_output_volume(id + 3, 0.5), OK);
    for (frame, muted) in [(0, false), (32, true)] {
        assert_eq!(fixture.registry.set_output_mute(id + 4, muted), OK);
        canonical.copy_from_slice(signal);
        assert!(unsafe {
            crate::abi::mr_hal_core_io(
                (&raw const *fixture.registry).cast_mut(),
                id,
                id + 2,
                true,
                1,
                frame,
                canonical.as_mut_ptr().cast(),
                32,
            )
        });
        bytes.fill(0xa5);
        native_block_io(fixture, id, (false, frame), bytes);
        let mut captured = vec![0.0; signal.len()];
        assert!(encoding.decode(bytes, &mut captured));
        let expected: Vec<_> = signal
            .iter()
            .map(|sample| {
                if muted {
                    0.0_f32.to_bits()
                } else {
                    (sample * 0.5).to_bits()
                }
            })
            .collect();
        assert_eq!(
            captured
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            expected
        );
    }
}

fn assert_native_capture(
    fixture: &Fixture,
    id: u32,
    _transport: &Transport,
    encoding: mixrelay_pcm_ring::SampleFormat,
    signal: &[f32],
) {
    // The real native render path above populated capture, including shared
    // gain and exact mute. No engine-side writer supplies these samples.
    let mut storage = vec![0.0_f32; signal.len()];
    // SAFETY: this exclusive initialized f32 allocation holds at least the encoded byte
    // extent and also supplies Float32 alignment. No f32 borrow coexists with bytes.
    let bytes = unsafe {
        std::slice::from_raw_parts_mut(
            storage.as_mut_ptr().cast::<u8>(),
            signal.len() * encoding.bytes_per_sample(),
        )
    };
    bytes.fill(0xa5);
    native_block_io(fixture, id, (false, 0), bytes);
    let mut captured = vec![0.0; signal.len()];
    assert!(encoding.decode(bytes, &mut captured));
    assert_eq!(
        captured
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>(),
        signal
            .iter()
            .map(|sample| (sample * 0.5).to_bits())
            .collect::<Vec<_>>()
    );
    bytes.fill(0xa5);
    native_block_io(fixture, id, (false, 32), bytes);
    assert!(encoding.decode(bytes, &mut captured));
    assert!(captured.iter().all(|sample| sample.to_bits() == 0));
}

fn start(fixture: &Fixture, device: u32) {
    assert_eq!(fixture.registry.client(device, 42, true), OK);
    assert_eq!(fixture.registry.start(device, 42, 1), OK);
}

#[test]
fn native_loopback_uses_absolute_callback_frames_in_both_callback_orders() {
    let fixture = native_fixture();
    let transport = Transport::new(10, 2, 4);
    let id = transport.add(&fixture, "Ordered loopback", 48_000);
    start(&fixture, id);
    let mut capture = [1.0_f32; 128];
    let read = |frame: u64, samples: &mut [f32; 128]| {
        assert!(unsafe {
            fixture.registry.io(IoOperation {
                client: 42,
                device: id,
                stream: id + 1,
                render: false,
                frame,
                samples: samples.as_mut_ptr().cast(),
                frames: 32,
            })
        });
    };
    for frame in [0, 32, 64] {
        read(frame, &mut capture);
        assert_eq!(capture.map(f32::to_bits), [0; 128]);
        let mut playback = [0.25_f32; 64];
        assert!(unsafe {
            fixture.registry.io(IoOperation {
                client: 42,
                device: id,
                stream: id + 2,
                render: true,
                frame,
                samples: playback.as_mut_ptr().cast(),
                frames: 32,
            })
        });
        read(frame, &mut capture);
        for channels in capture.chunks_exact(4) {
            assert_eq!(channels, [0.25, 0.25, 0.0, 0.0]);
        }
        // A consumer's different input sampletime cannot replay the last block.
        read(frame + 32, &mut capture);
        assert_eq!(capture.map(f32::to_bits), [0; 128]);
    }
}

#[test]
fn native_duplex_mappings_are_isolated_and_publish_independent_clocks() {
    let fixture = native_fixture();
    let first = Transport::new(10, 2, 2);
    let second = Transport::new(20, 7, 3);
    let a = first.add(&fixture, "Stereo", 48_000);
    let b = second.add(&fixture, "Seven", 96_000);
    start(&fixture, a);
    start(&fixture, b);
    let mut source_a = [0.25_f32; 64];
    let mut source_b = [0.75_f32; 224];
    for (device, samples) in [(a, source_a.as_mut_ptr()), (b, source_b.as_mut_ptr())] {
        assert!(unsafe {
            fixture.registry.io(IoOperation {
                client: 42,
                device,
                stream: device + 2,
                render: true,
                frame: 0,
                samples: samples.cast(),
                frames: 32,
            })
        });
    }
    assert_eq!(
        first.capture::<64>(&fixture, a, 32).map(f32::to_bits),
        source_a.map(f32::to_bits)
    );
    assert_eq!(
        second.capture::<96>(&fixture, b, 32).map(f32::to_bits),
        [0.75_f32.to_bits(); 96]
    );
    let mut capture_a = [0.0_f32; 64];
    let mut capture_b = [0.0_f32; 96];
    for (device, samples) in [(a, capture_a.as_mut_ptr()), (b, capture_b.as_mut_ptr())] {
        assert!(unsafe {
            fixture.registry.io(IoOperation {
                client: 42,
                device,
                stream: device + 1,
                render: false,
                frame: 0,
                samples: samples.cast(),
                frames: 32,
            })
        });
    }
    assert_eq!(capture_a.map(f32::to_bits), [0.25_f32.to_bits(); 64]);
    assert_eq!(capture_b.map(f32::to_bits), [0.75_f32.to_bits(); 96]);
    assert_eq!(
        (first.clock().sample_rate, second.clock().sample_rate),
        (48_000, 96_000)
    );
    assert_eq!(
        (first.clock().render_through, second.clock().capture_through),
        (32, 32)
    );
}

#[test]
fn native_replacement_retires_old_mapping_and_preserves_device_identity() {
    let fixture = native_fixture();
    let first = Transport::new(10, 2, 2);
    let id = first.add(&fixture, "Before", 48_000);
    start(&fixture, id);
    let original = fixture.registry.snapshot(id).unwrap();
    assert_eq!(fixture.registry.set_rate(id, 192_000), OK);
    assert_eq!(fixture.perform(), OK);
    let replacement = Transport::new(10, 7, 33);
    assert_eq!(replacement.add(&fixture, "After", 48_000), id);
    assert_eq!(fixture.registry.snapshot(id).unwrap().render_channels, 2);
    assert_eq!(fixture.perform(), OK);
    let current = fixture.registry.snapshot(id).unwrap();
    assert_eq!(original.uid, current.uid);
    assert_eq!(
        (
            current.render_channels,
            current.capture_channels,
            current.sample_rate
        ),
        (7, 33, 192_000)
    );
    assert_eq!(first.clock().running, 0);
    assert_eq!(
        (replacement.clock().running, replacement.clock().sample_rate),
        (1, 192_000)
    );
    // Core Audio restarts IO after a configuration change; that start maps the client's
    // queue in the new shape.
    assert_eq!(fixture.registry.stop(id, 42), OK);
    assert_eq!(fixture.registry.start(id, 42, 2), OK);
    let mut source = [0.75_f32; 224];
    assert!(unsafe {
        fixture.registry.io(IoOperation {
            client: 42,
            device: id,
            stream: id + 2,
            render: true,
            frame: 0,
            samples: source.as_mut_ptr().cast(),
            frames: 32,
        })
    });
    let captured = replacement.capture::<1056>(&fixture, id, 32);
    for frame in captured.chunks_exact(33) {
        assert!(
            frame[..7]
                .iter()
                .all(|sample| sample.to_bits() == 0.75_f32.to_bits())
        );
        assert!(frame[7..].iter().all(|sample| sample.to_bits() == 0));
    }
    let previous_epoch = replacement.clock().epoch;
    replacement.add(&fixture, "Same mapping", 48_000);
    assert_eq!(
        (replacement.clock().epoch, replacement.clock().running),
        (previous_epoch, 1)
    );
    assert_eq!(fixture.registry.stop(id, 42), OK);
    assert_eq!(replacement.clock().running, 0);
}

#[test]
fn authoritative_inventory_validates_every_entry_before_removal() {
    let fixture = native_fixture();
    let first = Transport::new(10, 2, 2);
    let second = Transport::new(20, 7, 3);
    let initial = Message::snapshot(
        &[(&first, "First", 48_000, 1), (&second, "Second", 96_000, 2)],
        1,
    );
    assert!(initial.apply(&fixture));
    let original = fixture.registry.devices();
    assert_eq!(original.len(), 2);
    let invalid = Message::snapshot(&[(&first, "Invalid", 384_000, 3)], 2);
    assert!(!invalid.apply(&fixture));
    assert_eq!(fixture.registry.devices(), original);
    let valid = Message::snapshot(&[(&first, "Renamed", 48_000, 3)], 2);
    assert!(valid.apply(&fixture));
    assert_eq!(fixture.registry.devices(), [original[0]]);
    assert_eq!(
        &fixture.registry.snapshot(original[0]).unwrap().name[..7],
        b"Renamed"
    );
    assert!(initial.apply(&fixture));
    assert_eq!(fixture.registry.devices(), [original[0]]);
    let empty = Message::snapshot(&[], 3);
    assert!(empty.apply(&fixture));
    assert!(fixture.registry.devices().is_empty());
}

#[test]
fn native_master_output_controls_scale_playback_and_loopback_at_product_widths() {
    for channels in [1, 7, 64] {
        let fixture = native_fixture();
        let transport = Transport::new(10, channels, channels);
        let id = transport.add(&fixture, "Native output controls", 48_000);
        start(&fixture, id);
        let count = channels as usize * 32;
        assert_eq!(fixture.registry.set_output_volume(id + 3, 0.5), OK);
        for (cycle, (muted, expected)) in [(false, 0.4_f32), (true, 0.0), (false, 0.4)]
            .into_iter()
            .enumerate()
        {
            let frame = u64::try_from(cycle).unwrap() * 32;
            assert_eq!(fixture.registry.set_output_mute(id + 4, muted), OK);
            let mut playback = [0.8_f32; 2048];
            assert!(unsafe {
                fixture.registry.io(IoOperation {
                    client: 42,
                    device: id,
                    stream: id + 2,
                    render: true,
                    frame,
                    samples: playback.as_mut_ptr().cast(),
                    frames: 32,
                })
            });
            assert!(
                playback[count..]
                    .iter()
                    .all(|sample| sample.to_bits() == 0.8_f32.to_bits())
            );
            let mut received = [0.0_f32; 2048];
            assert!(unsafe {
                fixture.registry.io(IoOperation {
                    client: 42,
                    device: id,
                    stream: id + 1,
                    render: false,
                    frame,
                    samples: received.as_mut_ptr().cast(),
                    frames: 32,
                })
            });
            assert!(
                received[..count]
                    .iter()
                    .all(|sample| sample.to_bits() == expected.to_bits())
            );
        }
        let replacement = Transport::new(10, channels, channels);
        assert_eq!(replacement.add(&fixture, "Reattached", 48_000), id);
        assert_eq!(fixture.perform(), OK);
        assert_eq!(fixture.registry.output_control(id), Some((0.5, false)));
    }
}
