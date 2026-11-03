use super::*;
use std::alloc::{Layout, alloc_zeroed, dealloc, handle_alloc_error};
use std::os::fd::{AsRawFd, OwnedFd};
use std::ptr::NonNull;

unsafe extern "C" {
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: i32,
        flags: i32,
        descriptor: i32,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> i32;
}

struct SharedMapping {
    memory: NonNull<c_void>,
    size: usize,
}
impl SharedMapping {
    fn new(descriptor: &OwnedFd, size: usize) -> Self {
        // SAFETY: descriptor owns a writable shared region of the stated size.
        let pointer = unsafe { mmap(ptr::null_mut(), size, 3, 1, descriptor.as_raw_fd(), 0) };
        assert_ne!(
            pointer as isize,
            -1,
            "mmap failed: {}",
            std::io::Error::last_os_error()
        );
        Self {
            memory: NonNull::new(pointer).unwrap(),
            size,
        }
    }
}
impl Drop for SharedMapping {
    fn drop(&mut self) {
        // SAFETY: every borrowed transport view has ended before this owner.
        assert_eq!(unsafe { munmap(self.memory.as_ptr(), self.size) }, 0);
    }
}

fn duplex() -> EndpointTransportConfig {
    EndpointTransportConfig {
        endpoint_id: [7; 16],
        capacity_frames: 16,
        render_channels: 2,
        capture_channels: 2,
    }
}

struct Mapping {
    memory: NonNull<u8>,
    layout: Layout,
    pointer: *mut EndpointTransport,
}
impl Mapping {
    fn new(config: &EndpointTransportConfig) -> Self {
        let size = unsafe { mr_endpoint_transport_required_bytes(config) };
        let layout = Layout::from_size_align(size, STORAGE_ALIGNMENT).unwrap();
        let memory = NonNull::new(unsafe { alloc_zeroed(layout) })
            .unwrap_or_else(|| handle_alloc_error(layout));
        // SAFETY: this raw allocation is exclusive, aligned and sufficiently sized.
        let pointer =
            unsafe { mr_endpoint_transport_initialize(memory.as_ptr().cast(), size, config) };
        assert!(!pointer.is_null());
        Self {
            memory,
            layout,
            pointer,
        }
    }
    fn snapshot(&self) -> Option<EndpointClockSnapshot> {
        let mut snapshot = EndpointClockSnapshot::default();
        unsafe { mr_endpoint_transport_clock_snapshot(self.pointer, &raw mut snapshot) }
            .then_some(snapshot)
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: no attached user survives this unique allocation owner.
        unsafe {
            dealloc(self.memory.as_ptr(), self.layout);
        }
    }
}

#[test]
fn endpoint_abi_and_invalid_configuration_contract() {
    assert_eq!(size_of::<EndpointTransport>(), 72);
    assert_eq!(size_of::<EndpointTransportConfig>(), 28);
    assert_eq!(size_of::<EndpointClock>(), 96);
    assert_eq!(size_of::<EndpointClockSnapshot>(), 40);
    let valid = duplex();
    assert_eq!(
        unsafe { mr_endpoint_transport_required_bytes(ptr::null()) },
        0
    );
    for config in [
        EndpointTransportConfig {
            endpoint_id: [0; 16],
            ..valid
        },
        EndpointTransportConfig {
            capacity_frames: 3,
            ..valid
        },
        EndpointTransportConfig {
            render_channels: 0,
            capture_channels: 0,
            ..valid
        },
        EndpointTransportConfig {
            render_channels: 65,
            ..valid
        },
    ] {
        assert_eq!(
            unsafe { mr_endpoint_transport_required_bytes(&raw const config) },
            0
        );
    }
    assert!(
        unsafe { mr_endpoint_transport_initialize(ptr::null_mut(), 0, &raw const valid) }.is_null()
    );
}

#[test]
fn independent_duplex_buffers_preserve_pcm_and_configuration() {
    let config = duplex();
    let mapping = Mapping::new(&config);
    let mut stored = EndpointTransportConfig::default();
    unsafe {
        assert!(mr_endpoint_transport_config(
            mapping.pointer,
            &raw mut stored
        ));
        assert_eq!(stored, config);
        assert_eq!(
            mr_endpoint_transport_attach(mapping.memory.as_ptr().cast(), mapping.layout.size()),
            mapping.pointer
        );
        let render = mr_endpoint_transport_render_buffer(mapping.pointer);
        let capture = mr_endpoint_transport_capture_buffer(mapping.pointer);
        assert!(!render.is_null() && !capture.is_null() && render != capture);
        for (buffer, frame, expected) in
            [(render, 12, [0.25_f32, -0.25]), (capture, 2, [0.75, -0.75])]
        {
            assert!(audio::mr_shared_audio_buffer_write(
                buffer,
                1,
                frame,
                expected.as_ptr(),
                1
            ));
            let mut output = [0.0; 2];
            assert_eq!(
                audio::mr_shared_audio_buffer_read(buffer, frame, output.as_mut_ptr(), 1),
                1
            );
            assert_eq!(output.map(f32::to_bits), expected.map(f32::to_bits));
            assert_eq!(audio::mr_shared_audio_buffer_stats(buffer).epoch, 1);
        }
    }
}

#[test]
fn one_way_mappings_and_null_accessors_do_not_invent_other_direction() {
    for (render_channels, capture_channels) in [(2, 0), (0, 3)] {
        let mapping = Mapping::new(&EndpointTransportConfig {
            render_channels,
            capture_channels,
            ..duplex()
        });
        unsafe {
            assert_eq!(
                mr_endpoint_transport_render_buffer(mapping.pointer).is_null(),
                render_channels == 0
            );
            assert_eq!(
                mr_endpoint_transport_capture_buffer(mapping.pointer).is_null(),
                capture_channels == 0
            );
        }
    }
    unsafe {
        assert!(mr_endpoint_transport_render_buffer(ptr::null_mut()).is_null());
        assert!(mr_endpoint_transport_capture_buffer(ptr::null_mut()).is_null());
    }
}

#[test]
fn attach_rejects_truncation_corrupt_extension_and_unsupported_shared_buffer_version() {
    let mapping = Mapping::new(&duplex());
    let memory = mapping.memory.as_ptr().cast();
    let size = mapping.layout.size();
    // SAFETY: mutations are exclusive, and restored before any other user starts.
    unsafe {
        assert!(mr_endpoint_transport_attach(memory, size - 1).is_null());
        let old_bytes = (*mapping.pointer).total_bytes;
        assert!(mr_endpoint_transport_attach(memory, old_bytes as usize).is_null());
        (*mapping.pointer).reserved = u32::MAX;
        assert!(mr_endpoint_transport_attach(memory, size).is_null());
        (*mapping.pointer).reserved = old_bytes;
        let clock = memory
            .cast::<u64>()
            .add(old_bytes as usize / 8)
            .cast::<EndpointClock>();
        (*clock).magic = 0;
        assert!(mr_endpoint_transport_attach(memory, size).is_null());
        (*clock).magic = CLOCK_MAGIC;
        let render = mr_endpoint_transport_render_buffer(mapping.pointer);
        // Shared-buffer version lives at byte8; immutable headers are corrupted
        // only in this exclusive test, never while atomics/readers are active.
        let version = render.cast::<u32>().add(2);
        let current_version = version.read();
        assert_eq!(current_version, 4);
        version.write(u32::MAX);
        assert!(mr_endpoint_transport_attach(memory, size).is_null());
        version.write(current_version);
        assert_eq!(mr_endpoint_transport_attach(memory, size), mapping.pointer);
        (*mapping.pointer).reserved = 0;
        assert!(mr_endpoint_transport_attach(memory, old_bytes as usize).is_null());
        assert!(mr_endpoint_transport_attach(memory, size).is_null());
        assert!(mapping.snapshot().is_none());
    }
}

#[test]
fn callback_quantum_changes_independently_of_accumulated_native_credit() {
    let mapping = Mapping::new(&duplex());
    // SAFETY: the test exclusively owns this initialized mapping; no callbacks
    // can race the clock transition or mapping destruction.
    unsafe {
        assert!(mr_endpoint_transport_begin_clock_transition(
            mapping.pointer
        ));
        mr_endpoint_transport_clock_state(mapping.pointer, 1, 48_000, true);
        mr_endpoint_transport_clock_demand(mapping.pointer, true, 128, 128);
        mr_endpoint_transport_clock_demand(mapping.pointer, false, 256, 256);
        mr_endpoint_transport_clock_demand(mapping.pointer, true, 4096, 32);
    }
    let clock = mapping.snapshot().unwrap();
    assert_eq!((clock.render_through, clock.render_frames), (4096, 32));
    assert_eq!((clock.capture_through, clock.capture_frames), (256, 256));
    unsafe { mr_endpoint_transport_clock_demand(mapping.pointer, false, 8192, 64) };
    let clock = mapping.snapshot().unwrap();
    assert_eq!((clock.render_through, clock.render_frames), (4096, 32));
    assert_eq!((clock.capture_through, clock.capture_frames), (8192, 64));
}

#[test]
fn native_clock_credits_are_absolute_coalesced_and_lifecycle_scoped() {
    let mapping = Mapping::new(&duplex());
    assert!(mapping.snapshot().is_none());
    unsafe {
        mr_endpoint_transport_clock_demand(mapping.pointer, true, 64, 64);
        assert!(mapping.snapshot().is_none());
        assert!(mr_endpoint_transport_begin_clock_transition(
            mapping.pointer
        ));
        mr_endpoint_transport_clock_state(mapping.pointer, 2, 48_000, true);
        mr_endpoint_transport_clock_demand(mapping.pointer, true, 128, 128);
        mr_endpoint_transport_clock_demand(mapping.pointer, true, 64, 64);
        mr_endpoint_transport_clock_demand(mapping.pointer, true, 0, 0);
        mr_endpoint_transport_clock_demand(mapping.pointer, true, u64::MAX, 1);
        mr_endpoint_transport_clock_demand(mapping.pointer, false, 256, 256);
    }
    assert_eq!(
        mapping.snapshot(),
        Some(EndpointClockSnapshot {
            epoch: 1,
            render_through: 128,
            capture_through: 256,
            sample_rate: 48_000,
            running: 1,
            capture_frames: 256,
            render_frames: 128,
        })
    );
    unsafe {
        assert!(mr_endpoint_transport_begin_clock_transition(
            mapping.pointer
        ));
        mr_endpoint_transport_clock_state(mapping.pointer, 3, 96_000, false);
    }
    let before = mapping.snapshot().unwrap();
    unsafe {
        mr_endpoint_transport_clock_demand(mapping.pointer, false, 1000, 32);
    }
    assert_eq!(mapping.snapshot(), Some(before));
    assert_eq!(
        before,
        EndpointClockSnapshot {
            epoch: 2,
            render_through: 0,
            capture_through: 0,
            sample_rate: 96_000,
            running: 0,
            capture_frames: 0,
            render_frames: 0,
        }
    );
}

#[test]
fn clock_transition_hides_all_intermediate_states_and_changes_observation_identity() {
    let mapping = Mapping::new(&duplex());
    unsafe {
        // Commit without acquiring the transition cannot create an observable clock.
        mr_endpoint_transport_clock_state(mapping.pointer, 1, 48_000, true);
        assert!(mapping.snapshot().is_none());
        assert!(mr_endpoint_transport_begin_clock_transition(
            mapping.pointer
        ));
        mr_endpoint_transport_clock_state(mapping.pointer, 1, 48_000, true);
        let before = mapping.snapshot().unwrap();
        assert!(mr_endpoint_transport_begin_clock_transition(
            mapping.pointer
        ));
        assert!(mapping.snapshot().is_none());
        mr_endpoint_transport_clock_demand(mapping.pointer, true, 999, 32);
        // Begin is idempotent only while the same control transaction is open.
        assert!(mr_endpoint_transport_begin_clock_transition(
            mapping.pointer
        ));
        assert!(mapping.snapshot().is_none());
        mr_endpoint_transport_clock_state(mapping.pointer, 1, 48_000, true);
        let after = mapping.snapshot().unwrap();
        assert_eq!(after.epoch, before.epoch + 1);
        assert_eq!(after.render_through, 0);
        assert_eq!(after.sample_rate, 48_000);
    }
}

#[test]
fn stale_clock_credit_cannot_change_new_lifecycle_demand() {
    let mapping = Mapping::new(&duplex());
    unsafe {
        assert!(mr_endpoint_transport_begin_clock_transition(
            mapping.pointer
        ));
        mr_endpoint_transport_clock_state(mapping.pointer, 1, 48_000, true);
        assert!(mr_endpoint_transport_begin_clock_transition(
            mapping.pointer
        ));
        mr_endpoint_transport_clock_state(mapping.pointer, 2, 44_100, true);
        let clock = (&*mapping.pointer).clock().unwrap();
        clock.render.publish((1 << 48) | 0x03e8, 32);
    }
    assert_eq!(mapping.snapshot().unwrap().render_through, 0);
    unsafe {
        mr_endpoint_transport_clock_demand(mapping.pointer, true, 32, 32);
    }
    assert_eq!(mapping.snapshot().unwrap().render_through, 32);
}

#[test]
fn full_native_epoch_is_preserved_and_missing_transition_cannot_overwrite_state() {
    let mapping = Mapping::new(&duplex());
    let native_epoch = u64::MAX - 1;
    unsafe {
        assert!(mr_endpoint_transport_begin_clock_transition(
            mapping.pointer
        ));
        mr_endpoint_transport_clock_state(mapping.pointer, native_epoch, 192_000, true);
        mr_endpoint_transport_clock_demand(mapping.pointer, true, 256, 256);
        let before = mapping.snapshot().unwrap();
        for _ in 0..100 {
            mr_endpoint_transport_clock_state(mapping.pointer, native_epoch, 192_000, true);
        }
        assert_eq!(mapping.snapshot(), Some(before));
        assert_eq!(before.epoch, 1);
        assert_eq!(before.render_through, 256);
        assert_eq!(
            (&*mapping.pointer)
                .clock()
                .unwrap()
                .native_epoch
                .load(Ordering::Acquire),
            native_epoch
        );
        // A changed tuple is a new lifecycle even when an input identity repeats.
        assert!(mr_endpoint_transport_begin_clock_transition(
            mapping.pointer
        ));
        mr_endpoint_transport_clock_state(mapping.pointer, native_epoch, 96_000, true);
        assert_eq!(mapping.snapshot().unwrap().epoch, 2);
        assert!(!mr_endpoint_transport_requires_replacement(mapping.pointer));
    }
}

#[test]
fn exhausted_clock_requires_new_mapping_without_reusing_remote_snapshot_tags() {
    let mapping = Mapping::new(&duplex());
    unsafe {
        for native_epoch in 1..=MAX_CLOCK_TAG {
            assert!(mr_endpoint_transport_begin_clock_transition(
                mapping.pointer
            ));
            mr_endpoint_transport_clock_state(mapping.pointer, native_epoch, 48_000, true);
        }
        let last = mapping.snapshot().unwrap();
        assert_eq!(last.epoch, MAX_CLOCK_TAG);
        // An identical last tuple is still allowed and preserves the final tag.
        mr_endpoint_transport_clock_state(mapping.pointer, MAX_CLOCK_TAG, 48_000, true);
        assert_eq!(mapping.snapshot(), Some(last));
        assert!(!mr_endpoint_transport_requires_replacement(mapping.pointer));
        assert!(mr_endpoint_transport_begin_clock_transition(
            mapping.pointer
        ));
        mr_endpoint_transport_clock_state(mapping.pointer, MAX_CLOCK_TAG + 1, 48_000, true);
        assert!(mr_endpoint_transport_requires_replacement(mapping.pointer));
        let mut output = last;
        assert!(!mr_endpoint_transport_clock_snapshot(
            mapping.pointer,
            &raw mut output
        ));
        assert_eq!(
            output, last,
            "failed snapshots must not publish a partial state"
        );
        mr_endpoint_transport_clock_demand(mapping.pointer, true, 1000, 32);
        mr_endpoint_transport_clock_state(mapping.pointer, 1, 48_000, true);
        assert!(mapping.snapshot().is_none());
        assert!(mr_endpoint_transport_requires_replacement(mapping.pointer));
        let fresh = Mapping::new(&duplex());
        assert!(mr_endpoint_transport_begin_clock_transition(fresh.pointer));
        mr_endpoint_transport_clock_state(fresh.pointer, MAX_CLOCK_TAG + 1, 48_000, true);
        assert_eq!(fresh.snapshot().unwrap().epoch, 1);
        assert!(!mr_endpoint_transport_requires_replacement(fresh.pointer));
        assert!(!mr_endpoint_transport_requires_replacement(ptr::null()));
    }
}

#[test]
fn pcm_identity_survives_deadline_contention_but_not_lifecycle_changes() {
    let mapping = Mapping::new(&duplex());
    // SAFETY: the fixture exclusively owns the initialized transport and clock;
    // no native publisher runs while the test simulates its odd sequence.
    unsafe {
        assert!(mr_endpoint_transport_begin_clock_transition(
            mapping.pointer
        ));
        mr_endpoint_transport_clock_state(mapping.pointer, 1, 48_000, true);
        let expected = mapping.snapshot().unwrap();
        let clock = (*mapping.pointer).clock().unwrap();
        clock.render.sequence.store(1, Ordering::Release);
        assert!(mapping.snapshot().is_none());
        assert!(clock_matches(mapping.pointer, &expected));
        clock.render.sequence.store(2, Ordering::Release);
        assert!(mr_endpoint_transport_begin_clock_transition(
            mapping.pointer
        ));
        assert!(!clock_matches(mapping.pointer, &expected));
        mr_endpoint_transport_clock_state(mapping.pointer, 2, 96_000, true);
        assert!(!clock_matches(mapping.pointer, &expected));
        let changed = mapping.snapshot().unwrap();
        assert!(clock_matches(mapping.pointer, &changed));
        assert!(mr_endpoint_transport_begin_clock_transition(
            mapping.pointer
        ));
        mr_endpoint_transport_clock_state(mapping.pointer, 3, 96_000, false);
        assert!(!clock_matches(mapping.pointer, &changed));
        assert!(!clock_matches(ptr::null(), &changed));
    }
}

#[test]
fn incarnation_identifies_mapping_across_real_fd_duplication_and_replacement() {
    let config = duplex();
    let size = unsafe { mr_endpoint_transport_required_bytes(&raw const config) };
    let descriptor = crate::anonymous_memory::create(size, true).unwrap();
    let first = SharedMapping::new(&descriptor, size);
    let original =
        unsafe { mr_endpoint_transport_initialize(first.memory.as_ptr(), size, &raw const config) };
    assert!(!original.is_null());
    let duplicate_descriptor = descriptor.try_clone().unwrap();
    let second = SharedMapping::new(&duplicate_descriptor, size);
    assert_ne!(first.memory, second.memory);
    let attached = unsafe { mr_endpoint_transport_attach(second.memory.as_ptr(), size) };
    assert!(!attached.is_null());
    let mut identity = [0; 16];
    let mut duplicate = [0; 16];
    unsafe {
        assert!(mr_endpoint_transport_incarnation(
            original,
            identity.as_mut_ptr()
        ));
        assert!(mr_endpoint_transport_incarnation(
            attached,
            duplicate.as_mut_ptr()
        ));
    }
    assert_ne!(identity, [0; 16]);
    assert_eq!(identity, duplicate);
    drop(descriptor);
    drop(duplicate_descriptor);
    let replacement = Mapping::new(&config);
    let mut new_identity = [0; 16];
    unsafe {
        assert!(mr_endpoint_transport_incarnation(
            replacement.pointer,
            new_identity.as_mut_ptr()
        ));
        assert_ne!(new_identity, identity);
        assert!(mr_endpoint_transport_begin_clock_transition(attached));
        mr_endpoint_transport_clock_state(attached, u64::MAX, 96_000, true);
        assert!(mr_endpoint_transport_incarnation(
            original,
            duplicate.as_mut_ptr()
        ));
        assert_eq!(
            duplicate, identity,
            "lifecycle changes cannot change mapping identity"
        );
        assert!(!mr_endpoint_transport_incarnation(
            ptr::null(),
            duplicate.as_mut_ptr()
        ));
        assert!(!mr_endpoint_transport_incarnation(
            original,
            ptr::null_mut()
        ));
    }
}
