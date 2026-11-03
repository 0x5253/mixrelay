use super::*;
use std::alloc::{Layout, alloc_zeroed, dealloc, handle_alloc_error};
use std::ptr::NonNull;
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn mmap(
        address: *mut c_void,
        size: usize,
        protection: i32,
        flags: i32,
        descriptor: i32,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, size: usize) -> i32;
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
    fn _exit(status: i32) -> !;
}

/// A real inherited `MAP_SHARED` region; no filesystem name or process-local
/// allocation is substituted for the interprocess producer claim under test.
struct DuplexMapping {
    memory: NonNull<c_void>,
    size: usize,
}
impl DuplexMapping {
    fn new() -> Self {
        use crate::endpoint_transport::{
            EndpointTransportConfig, mr_endpoint_transport_initialize,
            mr_endpoint_transport_required_bytes,
        };
        let config = EndpointTransportConfig {
            endpoint_id: [9; 16],
            capacity_frames: 8,
            render_channels: 1,
            capture_channels: 1,
        };
        let size = unsafe { mr_endpoint_transport_required_bytes(&raw const config) };
        // Darwin MAP_ANON|MAP_SHARED; prepare every allocation before fork.
        let memory = unsafe { mmap(ptr::null_mut(), size, 3, 0x1001, -1, 0) };
        assert_ne!(
            memory as isize,
            -1,
            "mmap failed: {}",
            std::io::Error::last_os_error()
        );
        let result = Self {
            memory: NonNull::new(memory).unwrap(),
            size,
        };
        assert!(
            !unsafe { mr_endpoint_transport_initialize(memory, size, &raw const config) }.is_null()
        );
        result
    }
    fn transport(&self) -> *mut crate::endpoint_transport::EndpointTransport {
        self.memory.as_ptr().cast()
    }
    fn render(&self) -> *mut SharedAudioBuffer {
        unsafe { crate::endpoint_transport::mr_endpoint_transport_render_buffer(self.transport()) }
    }
    fn capture(&self) -> *mut SharedAudioBuffer {
        unsafe { crate::endpoint_transport::mr_endpoint_transport_capture_buffer(self.transport()) }
    }
}
impl Drop for DuplexMapping {
    fn drop(&mut self) {
        // SAFETY: child is reaped and every borrowed HAL context has unbound.
        assert_eq!(unsafe { munmap(self.memory.as_ptr(), self.size) }, 0);
    }
}

/// Owns only the exact child PID returned by fork. Cleanup is bounded and never
/// signals a user process, a process group, or an already-reaped/reused PID.
struct ClaimChild(Option<i32>);
impl ClaimChild {
    fn spawn(buffer: *mut SharedAudioBuffer) -> Self {
        let pid = unsafe { fork() };
        assert!(pid >= 0, "fork failed: {}", std::io::Error::last_os_error());
        if pid == 0 {
            // SAFETY: inherited live MAP_SHARED bytes were initialized before
            // fork. This branch performs only lock-free atomic claim and _exit:
            // no allocation, XPC, printing, unwinding or Rust destructors run.
            let claim = unsafe { &*buffer }.claim_producer();
            if let Some(claim) = claim {
                std::mem::forget(claim);
                unsafe { _exit(0) }
            }
            unsafe { _exit(2) }
        }
        Self(Some(pid))
    }
    fn poll(&mut self) -> Option<i32> {
        let pid = self.0?;
        let mut status = 0;
        let result = unsafe { waitpid(pid, &raw mut status, 1) }; // WNOHANG.
        if result == pid {
            self.0 = None;
            return Some(status);
        }
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                // ECHILD means no longer ours to signal; other errors also must
                // not turn an unverified PID into a cleanup target.
                self.0 = None;
                panic!("waitpid failed: {error}");
            }
        }
        None
    }
    fn wait(&mut self) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(status) = self.poll() {
                return status;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("owned claim child did not exit within five seconds");
    }
}
impl Drop for ClaimChild {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            // This unreaped exact child cannot have its PID reassigned yet.
            unsafe {
                kill(pid, 9);
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.0.is_some() && Instant::now() < deadline {
                if self.poll().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(
                self.0.is_none(),
                "failed to reap owned claim child after SIGKILL"
            );
        }
    }
}

/// Owns raw aligned storage, never a non-atomic Rust slice alias to live PCM.
struct Buffer {
    memory: NonNull<u8>,
    layout: Layout,
    pointer: *mut SharedAudioBuffer,
}

// SAFETY: immutable geometry and atomic shared cells are the only accesses after
// construction. Tests use exactly one PCM writer; scoped threads join before Drop.
unsafe impl Sync for Buffer {}

impl Buffer {
    fn new(capacity: u32, channels: u32) -> Self {
        let size = mr_shared_audio_buffer_required_bytes(capacity, channels);
        let layout = Layout::from_size_align(size, align_of::<SharedAudioBuffer>()).unwrap();
        let memory = NonNull::new(unsafe { alloc_zeroed(layout) })
            .unwrap_or_else(|| handle_alloc_error(layout));
        // SAFETY: this allocation is exclusive, aligned and exactly large enough.
        let pointer = unsafe {
            mr_shared_audio_buffer_initialize(memory.as_ptr().cast(), size, capacity, channels)
        };
        assert!(!pointer.is_null());
        Self {
            memory,
            layout,
            pointer,
        }
    }
    fn header(&self) -> &SharedAudioBuffer {
        // SAFETY: header lives until this owner drops, geometry is immutable.
        unsafe { &*self.pointer }
    }
    fn write(&self, start: u64, samples: &[f32]) -> bool {
        let channels = self.header().channels as usize;
        assert_eq!(samples.len() % channels, 0);
        unsafe {
            mr_shared_audio_buffer_write(
                self.pointer,
                self.stats().epoch,
                start,
                samples.as_ptr(),
                u32::try_from(samples.len() / channels).unwrap(),
            )
        }
    }
    fn read(&self, start: u64, samples: &mut [f32]) -> u32 {
        let channels = self.header().channels as usize;
        assert_eq!(samples.len() % channels, 0);
        unsafe {
            mr_shared_audio_buffer_read(
                self.pointer,
                start,
                samples.as_mut_ptr(),
                u32::try_from(samples.len() / channels).unwrap(),
            )
        }
    }
    fn stats(&self) -> SharedAudioBufferStats {
        unsafe { mr_shared_audio_buffer_stats(self.pointer) }
    }
    fn read_coverage(&self, start: u64, samples: &mut [f32], coverage: &mut [bool]) -> u32 {
        // SAFETY: this owner retains the mapping; each reader owns disjoint output slices.
        unsafe { read_with_frame_coverage(self.pointer, start, samples, coverage) }
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: every user/thread borrows this owner and has finished. No live
        // atomic references survive; allocation and deallocation layouts match.
        unsafe {
            dealloc(self.memory.as_ptr(), self.layout);
        }
    }
}

#[test]
fn abi_layout_and_geometry_are_bounded() {
    assert_eq!(size_of::<SharedAudioBuffer>(), 80);
    assert_eq!(size_of::<SharedAudioBufferStats>(), 40);
    assert_eq!(
        std::mem::offset_of!(SharedAudioBuffer, published_through_frame),
        64
    );
    assert_eq!(mr_shared_audio_buffer_required_bytes(4, 2), 144);
    assert_ne!(
        mr_shared_audio_buffer_required_bytes(MAX_CAPACITY, MAX_CHANNELS),
        0
    );
    for (capacity, channels) in [(0, 2), (3, 2), (MAX_CAPACITY * 2, 2), (4, 0), (4, 65)] {
        assert_eq!(mr_shared_audio_buffer_required_bytes(capacity, channels), 0);
    }
}

#[test]
fn initialization_and_attach_reject_invalid_or_old_geometry() {
    let buffer = Buffer::new(8, 2);
    let memory = buffer.memory.as_ptr().cast();
    let size = buffer.layout.size();
    // SAFETY: all tested pointers are either null or inside the owned allocation.
    // Corruptions are exclusive and restored before any PCM/atomic user resumes.
    unsafe {
        assert!(mr_shared_audio_buffer_initialize(ptr::null_mut(), size, 8, 2).is_null());
        assert!(mr_shared_audio_buffer_initialize(memory, size - 1, 8, 2).is_null());
        assert!(
            mr_shared_audio_buffer_initialize(buffer.memory.as_ptr().add(1).cast(), size - 1, 8, 2)
                .is_null()
        );
        assert_eq!(mr_shared_audio_buffer_attach(memory, size), buffer.pointer);
        assert!(mr_shared_audio_buffer_attach(memory, size - 1).is_null());
        assert!(mr_shared_audio_buffer_attach(memory, 71).is_null());
        assert!(mr_shared_audio_buffer_attach(ptr::null_mut(), size).is_null());
        (*buffer.pointer).version = 2;
        assert!(mr_shared_audio_buffer_attach(memory, size).is_null());
        (*buffer.pointer).version = VERSION;
        (*buffer.pointer).capacity_mask = 0;
        assert!(mr_shared_audio_buffer_attach(memory, size).is_null());
        (*buffer.pointer).capacity_mask = 7;
        (*buffer.pointer).magic = 0;
        assert!(mr_shared_audio_buffer_attach(memory, size).is_null());
        (*buffer.pointer).magic = MAGIC;
        assert_eq!(mr_shared_audio_buffer_attach(memory, size), buffer.pointer);
    }
}

#[test]
fn zeroed_storage_keeps_its_ring_untouched_and_reused_storage_is_cleared() {
    let (capacity, channels) = (8, 2);
    let size = mr_shared_audio_buffer_required_bytes(capacity, channels);
    let layout = Layout::from_size_align(size, align_of::<SharedAudioBuffer>()).unwrap();
    let stamps = size_of::<SharedAudioBuffer>();
    let pcm = stamps + capacity as usize * size_of::<u64>();
    let read = |buffer: *mut SharedAudioBuffer| {
        let mut samples = [9.0; 16];
        let mut coverage = [true; 8];
        // SAFETY: the test owns the initialized allocation and disjoint outputs.
        let available = unsafe { read_with_frame_coverage(buffer, 0, &mut samples, &mut coverage) };
        (available, samples.map(f32::to_bits), coverage)
    };
    // SAFETY: each allocation is exclusive, aligned and exactly the ring's extent.
    unsafe {
        let memory = alloc_zeroed(layout);
        assert!(!memory.is_null());
        // The PCM cells hold a sentinel that a full clear would erase. Zero stamps alone
        // must reject every frame, so the zeroed initializer leaves the ring untouched.
        memory.add(pcm).write_bytes(0x5a, size - pcm);
        let buffer = initialize_zeroed(memory.cast(), size, capacity, channels);
        assert!(!buffer.is_null());
        assert!((pcm..size).all(|offset| memory.add(offset).read() == 0x5a));
        assert_eq!(read(buffer), (0, [0; 16], [false; 8]));
        dealloc(memory, layout);

        // Reused storage may contain stamps of the epoch a new header starts at.
        let memory = alloc_zeroed(layout);
        assert!(!memory.is_null());
        for slot in 0..capacity as usize {
            memory
                .add(stamps + slot * size_of::<u64>())
                .cast::<[u8; 8]>()
                .write(stamp(1, slot as u64).to_ne_bytes());
        }
        memory.add(pcm).write_bytes(0x5a, size - pcm);
        let buffer = mr_shared_audio_buffer_initialize(memory.cast(), size, capacity, channels);
        assert!(!buffer.is_null());
        assert_eq!(read(buffer), (0, [0; 16], [false; 8]));
        dealloc(memory, layout);
    }
}

#[test]
fn pcm_bits_activity_and_counters_roundtrip() {
    let buffer = Buffer::new(8, 2);
    let bits = [0x8000_0000, 0x7fc0_1234, 0x3f00_0000, 0xbf80_0000];
    let input = bits.map(f32::from_bits);
    assert!(buffer.write(100, &input));
    let mut output = [0.0; 4];
    assert_eq!(buffer.read(100, &mut output), 2);
    assert_eq!(output.map(f32::to_bits), bits);
    assert_eq!(
        buffer.stats(),
        SharedAudioBufferStats {
            written_frames: 2,
            read_frames: 2,
            missing_frames: 0,
            epoch: 1,
            published_through_frame: 102
        }
    );
    unsafe {
        assert!(mr_shared_audio_buffer_is_lock_free(buffer.pointer));
        assert_eq!(mr_shared_audio_buffer_capacity(buffer.pointer), 8);
        assert_eq!(mr_shared_audio_buffer_channels(buffer.pointer), 2);
        for activity in 0..=3 {
            mr_shared_audio_buffer_set_producer_activity(buffer.pointer, activity);
            assert_eq!(
                mr_shared_audio_buffer_producer_activity(buffer.pointer),
                activity
            );
        }
        mr_shared_audio_buffer_set_producer_activity(buffer.pointer, 99);
        assert_eq!(mr_shared_audio_buffer_producer_activity(buffer.pointer), 0);
        assert_eq!(
            mr_shared_audio_buffer_stats(ptr::null()),
            SharedAudioBufferStats::default()
        );
    }
}

#[test]
fn frame_coverage_distinguishes_committed_zero_missing_wrap_and_epoch_retirement() {
    let buffer = Buffer::new(8, 2);
    assert!(buffer.write(6, &[0.0, -0.0, 0.25, -0.25]));
    assert!(buffer.write(9, &[0.5, -0.5]));
    let mut samples = [9.0; 14];
    let mut coverage = [true; 7];
    let read = |samples: &mut [f32], coverage: &mut [bool]| {
        // SAFETY: Buffer owns the initialized mapping and disjoint output slices.
        unsafe { read_with_frame_coverage(buffer.pointer, 5, samples, coverage) }
    };
    assert_eq!(read(&mut samples, &mut coverage), 3);
    assert_eq!(coverage, [false, true, true, false, true, false, false]);
    assert_eq!(
        samples.map(f32::to_bits),
        [
            0.0_f32, 0.0, 0.0, -0.0, 0.25, -0.25, 0.0, 0.0, 0.5, -0.5, 0.0, 0.0, 0.0, 0.0
        ]
        .map(f32::to_bits)
    );
    assert_eq!(buffer.stats().read_frames, 7);
    assert_eq!(buffer.stats().missing_frames, 4);
    let mut legacy = [9.0; 14];
    assert_eq!(buffer.read(5, &mut legacy), 3);
    assert_eq!(legacy.map(f32::to_bits), samples.map(f32::to_bits));
    // SAFETY: the owner excludes writes while requesting retirement.
    unsafe {
        mr_shared_audio_buffer_request_epoch(buffer.pointer);
    }
    assert_eq!(read(&mut samples, &mut coverage), 0);
    assert_eq!(samples.map(f32::to_bits), [0; 14]);
    assert_eq!(coverage, [false; 7]);
    // SAFETY: no producer is active; the owner opens the next timeline.
    assert_eq!(
        unsafe { mr_shared_audio_buffer_try_begin_epoch(buffer.pointer) },
        2
    );
    assert!(buffer.write(6, &[0.75; 2]));
    assert_eq!(read(&mut samples, &mut coverage), 1);
    assert_eq!(coverage, [false, true, false, false, false, false, false]);
    let before = buffer.stats();
    assert_eq!(read(&mut samples[..12], &mut coverage), 0);
    assert_eq!(coverage, [false; 7]);
    assert_eq!(buffer.stats(), before);
}

#[test]
fn ring_overwrite_silences_only_missing_absolute_frames() {
    let buffer = Buffer::new(4, 1);
    assert!(buffer.write(0, &[1.0, 2.0, 3.0, 4.0]));
    assert!(buffer.write(4, &[5.0, 6.0]));
    let mut output = [9.0; 4];
    assert_eq!(buffer.read(0, &mut output), 2);
    assert_eq!(
        output.map(f32::to_bits),
        [0.0_f32, 0.0, 3.0, 4.0].map(f32::to_bits)
    );
    assert_eq!(buffer.stats().missing_frames, 2);
}

#[test]
fn pending_request_is_idempotent_and_blocks_old_pcm_until_writer_rearms() {
    let buffer = Buffer::new(8, 1);
    assert!(buffer.write(100, &[0.5; 4]));
    unsafe {
        assert_eq!(mr_shared_audio_buffer_request_epoch(buffer.pointer), 1);
        assert_eq!(mr_shared_audio_buffer_request_epoch(buffer.pointer), 1);
        assert!(mr_shared_audio_buffer_epoch_pending(buffer.pointer));
        assert!(!mr_shared_audio_buffer_requires_replacement(buffer.pointer));
        assert_eq!(mr_shared_audio_buffer_published_through(buffer.pointer), 0);
    }
    assert_eq!(buffer.stats().epoch, 0);
    assert!(!buffer.write(104, &[1.0]));
    let mut output = [9.0; 4];
    assert_eq!(buffer.read(100, &mut output), 0);
    assert!(output.iter().all(|sample| sample.to_bits() == 0));
    unsafe {
        assert_eq!(mr_shared_audio_buffer_try_begin_epoch(buffer.pointer), 2);
        assert!(!mr_shared_audio_buffer_epoch_pending(buffer.pointer));
    }
    assert_eq!(buffer.read(100, &mut output), 0);
    assert!(buffer.write(0, &[0.25; 4]));
    assert_eq!(buffer.read(0, &mut output), 4);
    assert!(
        output
            .iter()
            .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
    );
}

#[test]
fn exhausted_mapping_never_reuses_a_remote_readers_generation() {
    let old = Buffer::new(4, 1);
    assert!(old.write(0, &[0.75]));
    for expected in 2..=MAX_EPOCH {
        assert_eq!(
            unsafe { mr_shared_audio_buffer_try_begin_epoch(old.pointer) },
            expected
        );
    }
    assert_eq!(
        unsafe { mr_shared_audio_buffer_try_begin_epoch(old.pointer) },
        0
    );
    assert_eq!(
        unsafe { mr_shared_audio_buffer_try_begin_epoch(old.pointer) },
        0
    );
    assert!(unsafe { mr_shared_audio_buffer_epoch_pending(old.pointer) });
    assert!(unsafe { mr_shared_audio_buffer_requires_replacement(old.pointer) });
    // The generation-one stamp is not cleared or reused in this live mapping.
    assert_eq!(
        unsafe { old.header().stamp_cell(0) }.load(Ordering::Acquire),
        stamp(1, 0)
    );
    let fresh = Buffer::new(4, 1);
    let mut output = [1.0];
    assert_eq!(fresh.read(0, &mut output), 0);
    assert_eq!(output[0].to_bits(), 0);
    assert_eq!(fresh.stats().epoch, 1);
    assert!(!unsafe { mr_shared_audio_buffer_requires_replacement(fresh.pointer) });
    assert_eq!(old.read(0, &mut output), 0);
}

#[test]
fn replacement_signal_distinguishes_pending_rearm_from_exhaustion() {
    let buffer = Buffer::new(4, 1);
    assert!(!unsafe { mr_shared_audio_buffer_requires_replacement(ptr::null()) });
    assert!(!unsafe { mr_shared_audio_buffer_requires_replacement(buffer.pointer) });
    // Deterministic boundary setup uses only an atomic store; no PCM writer or
    // reader is active while this test moves the generation near exhaustion.
    buffer
        .header()
        .epoch
        .store(MAX_EPOCH - 1, Ordering::Release);
    unsafe {
        assert_eq!(
            mr_shared_audio_buffer_request_epoch(buffer.pointer),
            MAX_EPOCH - 1
        );
        assert!(mr_shared_audio_buffer_epoch_pending(buffer.pointer));
        assert!(!mr_shared_audio_buffer_requires_replacement(buffer.pointer));
        assert_eq!(
            mr_shared_audio_buffer_try_begin_epoch(buffer.pointer),
            MAX_EPOCH
        );
        assert!(!mr_shared_audio_buffer_epoch_pending(buffer.pointer));
        assert!(!mr_shared_audio_buffer_requires_replacement(buffer.pointer));
        assert_eq!(
            mr_shared_audio_buffer_request_epoch(buffer.pointer),
            MAX_EPOCH
        );
        assert!(mr_shared_audio_buffer_requires_replacement(buffer.pointer));
        assert_eq!(mr_shared_audio_buffer_try_begin_epoch(buffer.pointer), 0);
        assert_eq!(mr_shared_audio_buffer_try_begin_epoch(buffer.pointer), 0);
        assert!(mr_shared_audio_buffer_requires_replacement(buffer.pointer));
    }
    assert_eq!(
        buffer.header().epoch.load(Ordering::Acquire),
        REQUESTED | MAX_EPOCH
    );
}

#[test]
fn stale_writer_watermark_cannot_become_a_new_epoch_timeline() {
    let buffer = Buffer::new(4, 1);
    assert!(buffer.write(100, &[1.0]));
    assert_eq!(
        unsafe { mr_shared_audio_buffer_try_begin_epoch(buffer.pointer) },
        2
    );
    // Deterministic interleaving: an old writer reaches publication after rearm.
    buffer
        .header()
        .published_through_frame
        .store(stamp(1, 101), Ordering::Release);
    assert_eq!(buffer.stats().epoch, 2);
    assert_eq!(buffer.stats().published_through_frame, 0);
    assert_eq!(
        unsafe { mr_shared_audio_buffer_published_through(buffer.pointer) },
        0
    );
    assert!(buffer.write(0, &[2.0]));
    assert_eq!(buffer.stats().published_through_frame, 1);
}

#[test]
fn rearm_after_observation_rejects_old_format_pcm_before_publication() {
    let buffer = Buffer::new(4, 1);
    let observed_epoch = buffer.stats().epoch;
    assert_eq!(
        unsafe { mr_shared_audio_buffer_try_begin_epoch(buffer.pointer) },
        2
    );
    let old_format_samples = [0.75; 2];
    assert!(!unsafe {
        mr_shared_audio_buffer_write(
            buffer.pointer,
            observed_epoch,
            0,
            old_format_samples.as_ptr(),
            2,
        )
    });
    assert_eq!(buffer.stats().written_frames, 0);
    assert_eq!(buffer.stats().published_through_frame, 0);
    let mut output = [1.0; 2];
    assert_eq!(buffer.read(0, &mut output), 0);
    assert!(output.iter().all(|sample| sample.to_bits() == 0));
    assert!(
        buffer.write(0, &[0.25; 2]),
        "failed expected-epoch check releases its producer claim"
    );
    assert_eq!(buffer.read(0, &mut output), 2);
    assert!(
        output
            .iter()
            .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
    );
}

#[test]
fn active_producer_defers_control_rearm_without_exposing_a_new_epoch() {
    let buffer = Buffer::new(4, 1);
    let observed_epoch = buffer.stats().epoch;
    // Deterministically hold the same RAII claim used inside production writes.
    // Control must invalidate, not advance, while that producer is still active.
    let producer = buffer.header().claim_producer().unwrap();
    assert_eq!(
        unsafe { mr_shared_audio_buffer_try_begin_epoch(buffer.pointer) },
        0
    );
    assert_eq!(
        buffer.header().epoch.load(Ordering::Acquire),
        REQUESTED | observed_epoch
    );
    assert_eq!(buffer.stats().epoch, 0);
    assert!(!unsafe {
        mr_shared_audio_buffer_write(buffer.pointer, observed_epoch, 0, [1.0].as_ptr(), 1)
    });
    drop(producer);
    assert_eq!(
        unsafe { mr_shared_audio_buffer_try_begin_epoch(buffer.pointer) },
        2
    );
    assert!(!unsafe {
        mr_shared_audio_buffer_write(buffer.pointer, observed_epoch, 0, [1.0].as_ptr(), 1)
    });
    assert!(buffer.write(0, &[0.5]));
    assert_eq!(buffer.stats().written_frames, 1);
}

#[test]
fn rejected_write_paths_release_the_claim_and_never_accept_inactive_epoch() {
    let buffer = Buffer::new(4, 1);
    for expected_epoch in [0, REQUESTED | 1, MAX_EPOCH + 1, 2] {
        assert!(!unsafe {
            mr_shared_audio_buffer_write(buffer.pointer, expected_epoch, 0, [1.0].as_ptr(), 1)
        });
        assert_eq!(buffer.header().producer_busy.load(Ordering::Acquire), 0);
    }
    assert!(buffer.write(0, &[0.5]));
    assert!(!buffer.write(0, &[0.75]));
    assert_eq!(buffer.header().producer_busy.load(Ordering::Acquire), 0);
    assert_eq!(
        unsafe { mr_shared_audio_buffer_try_begin_epoch(buffer.pointer) },
        2
    );
}

#[test]
fn exited_hal_writer_is_retired_in_both_rings_and_only_new_mapping_recovers() {
    use crate::hal_io::{
        HalIoContext, mr_hal_io_begin_stream, mr_hal_io_bind, mr_hal_io_requires_replacement,
        mr_hal_io_unbind,
    };
    for capture in [false, true] {
        let old = DuplexMapping::new();
        let ring = if capture { old.capture() } else { old.render() };
        let mut child = ClaimChild::spawn(ring);
        assert_eq!(
            child.wait(),
            0,
            "child must exit after acquiring its real claim"
        );
        assert_eq!(unsafe { &*ring }.producer_busy.load(Ordering::Acquire), 1);
        let mut restarted_hal = HalIoContext::new(1, 1).unwrap();
        unsafe {
            assert!(mr_hal_io_bind(
                &raw mut restarted_hal,
                old.transport().cast()
            ));
            // Actual native caller authority: the old producer process has exited,
            // and this replacement context has never admitted any IO callbacks.
            assert!(!mr_hal_io_begin_stream(&raw mut restarted_hal));
            assert!(mr_hal_io_requires_replacement(&raw mut restarted_hal));
            assert!(mr_shared_audio_buffer_requires_replacement(ring));
            assert_eq!(mr_shared_audio_buffer_try_begin_epoch(ring), 0);
            assert!(!mr_shared_audio_buffer_write(ring, 1, 0, [1.0].as_ptr(), 1));
            assert_eq!(
                (&*ring).producer_busy.load(Ordering::Acquire),
                1,
                "retirement must never clear an abandoned claim"
            );
            mr_hal_io_unbind(&raw mut restarted_hal);
            let fresh = DuplexMapping::new();
            assert!(mr_hal_io_bind(
                &raw mut restarted_hal,
                fresh.transport().cast()
            ));
            assert!(mr_hal_io_begin_stream(&raw mut restarted_hal));
            assert!(!mr_hal_io_requires_replacement(&raw mut restarted_hal));
            assert!(mr_shared_audio_buffer_write(
                fresh.render(),
                mr_shared_audio_buffer_stats(fresh.render()).epoch,
                0,
                [0.25].as_ptr(),
                1
            ));
            mr_hal_io_unbind(&raw mut restarted_hal);
        }
    }
}

#[test]
fn capture_rearm_never_retires_a_live_hal_writer_and_recovers_after_claim_release() {
    use crate::hal_io::{
        HalIoContext, mr_hal_io_begin_stream, mr_hal_io_bind, mr_hal_io_clock_state,
        mr_hal_io_requires_replacement, mr_hal_io_unbind,
    };
    let mapping = DuplexMapping::new();
    let capture_pointer = mapping.capture();
    let capture = unsafe { &*capture_pointer };
    let mut hal = HalIoContext::new(1, 1).unwrap();
    unsafe {
        assert!(mr_hal_io_bind(&raw mut hal, mapping.transport().cast()));
        assert!(mr_hal_io_begin_stream(&raw mut hal));
        mr_hal_io_clock_state(&raw mut hal, 48_000, true);
        assert!(mr_shared_audio_buffer_write(
            capture_pointer,
            mr_shared_audio_buffer_stats(capture_pointer).epoch,
            0,
            [0.25_f32].as_ptr(),
            1
        ));
    }
    let epoch = unsafe { mr_shared_audio_buffer_stats(capture_pointer) }.epoch;
    // Model the sole HAL capture publication still owning its claim. Reader-side rearm may
    // request silence concurrently, but drained-only begin_stream has no authority here.
    let hal_writer = capture.claim_producer().unwrap();
    let mut output = [1.0_f32];
    let pcm = [0.5_f32];
    unsafe {
        assert_eq!(mr_shared_audio_buffer_try_begin_epoch(capture_pointer), 0);
        assert!(!mr_hal_io_requires_replacement(&raw mut hal));
        assert!(!mr_shared_audio_buffer_requires_replacement(
            capture_pointer
        ));
        assert!(mr_shared_audio_buffer_epoch_pending(capture_pointer));
        assert_eq!(capture.producer_busy.load(Ordering::Acquire), 1);
        assert!(!mr_shared_audio_buffer_write(
            capture_pointer,
            epoch,
            1,
            pcm.as_ptr(),
            1
        ));
        assert_eq!(
            mr_shared_audio_buffer_read(capture_pointer, 0, output.as_mut_ptr(), 1),
            0
        );
        assert_eq!(output.map(f32::to_bits), [0.0_f32.to_bits()]);
    }
    drop(hal_writer);
    assert_eq!(
        unsafe { mr_shared_audio_buffer_try_begin_epoch(capture_pointer) },
        epoch + 1
    );
    unsafe {
        assert!(!mr_shared_audio_buffer_write(
            capture_pointer,
            epoch,
            1,
            pcm.as_ptr(),
            1
        ));
        assert!(mr_shared_audio_buffer_write(
            capture_pointer,
            epoch + 1,
            1,
            pcm.as_ptr(),
            1
        ));
        assert_eq!(
            mr_shared_audio_buffer_read(capture_pointer, 1, output.as_mut_ptr(), 1),
            1
        );
        assert_eq!(output.map(f32::to_bits), [0.5_f32.to_bits()]);
        mr_hal_io_unbind(&raw mut hal);
    }
    assert!(!unsafe { mr_shared_audio_buffer_retire_busy_producer_after_drain(capture_pointer) });
    assert!(!unsafe { mr_shared_audio_buffer_retire_busy_producer_after_drain(ptr::null_mut()) });
}

#[test]
fn packed_end_overflow_and_same_epoch_stamp_aba_are_rejected() {
    let buffer = Buffer::new(4, 1);
    assert!(buffer.write(10, &[1.0, 2.0]));
    assert!(!buffer.write(10, &[9.0]));
    assert!(!buffer.write(11, &[9.0, 9.0]));
    let mut output = [0.0; 2];
    assert_eq!(buffer.read(10, &mut output), 2);
    assert_eq!(output.map(f32::to_bits), [1.0_f32, 2.0].map(f32::to_bits));
    assert!(buffer.write(FRAME_MASK - 1, &[3.0]));
    assert_eq!(buffer.stats().published_through_frame, FRAME_MASK);
    assert!(!buffer.write(FRAME_MASK, &[4.0]));
    assert!(!buffer.write(u64::MAX, &[4.0, 5.0]));
    assert_eq!(buffer.read(FRAME_MASK, &mut output[..1]), 0);
    unsafe {
        assert!(mr_shared_audio_buffer_write(
            buffer.pointer,
            buffer.stats().epoch,
            u64::MAX,
            ptr::null(),
            0
        ));
        assert_eq!(
            mr_shared_audio_buffer_read(buffer.pointer, u64::MAX, ptr::null_mut(), 0),
            0
        );
    }
}

#[test]
fn concurrent_bulk_readers_report_only_complete_frames_during_ring_replacement() {
    const CHANNELS: usize = 8;
    const FRAMES: u32 = 20_000;
    let buffer = Buffer::new(64, u32::try_from(CHANNELS).unwrap());
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for frame in 0..FRAMES {
                let samples = std::array::from_fn::<_, CHANNELS, _>(|channel| {
                    f32::from_bits(0x3f00_0000 + frame * 16 + u32::try_from(channel).unwrap())
                });
                assert!(buffer.write(u64::from(frame), &samples));
            }
        });
        for _ in 0..2 {
            scope.spawn(|| {
                for _ in 0..FRAMES {
                    let start = buffer.stats().published_through_frame.saturating_sub(16);
                    let mut samples = [0.0; 16 * CHANNELS];
                    let mut coverage = [false; 16];
                    let available = buffer.read_coverage(start, &mut samples, &mut coverage);
                    assert_eq!(
                        available as usize,
                        coverage.iter().filter(|valid| **valid).count()
                    );
                    for (index, frame) in samples.chunks_exact(CHANNELS).enumerate() {
                        for (channel, sample) in frame.iter().enumerate() {
                            let expected = if coverage[index] {
                                0x3f00_0000
                                    + u32::try_from(start + index as u64).unwrap() * 16
                                    + u32::try_from(channel).unwrap()
                            } else {
                                0
                            };
                            assert_eq!(sample.to_bits(), expected);
                        }
                    }
                }
            });
        }
    });
}

#[test]
fn concurrent_ring_replacement_never_returns_torn_multichannel_pcm() {
    const CHANNELS: usize = 8;
    const FRAMES: u32 = 20_000;
    let buffer = Buffer::new(64, u32::try_from(CHANNELS).unwrap());
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for frame in 0..FRAMES {
                // Exact bit patterns avoid float rounding ambiguity in this test.
                let samples = std::array::from_fn::<_, CHANNELS, _>(|channel| {
                    f32::from_bits(0x3f00_0000 + frame * 16 + u32::try_from(channel).unwrap())
                });
                assert!(buffer.write(u64::from(frame), &samples));
            }
        });
        for _ in 0..FRAMES {
            let end = unsafe { mr_shared_audio_buffer_published_through(buffer.pointer) };
            if end == 0 {
                continue;
            }
            let frame = end - 1;
            let mut samples = [0.0; CHANNELS];
            let available = buffer.read(frame, &mut samples);
            for (channel, sample) in samples.iter().enumerate() {
                let expected = if available == 0 {
                    0
                } else {
                    0x3f00_0000
                        + u32::try_from(frame).unwrap() * 16
                        + u32::try_from(channel).unwrap()
                };
                assert_eq!(sample.to_bits(), expected);
            }
        }
    });
    assert_eq!(buffer.stats().written_frames, u64::from(FRAMES));
}
