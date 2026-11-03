//! Application producer/consumer clock contract. All mutable fields are atomic;
//! no Rust references to non-atomic interprocess-writable data are exposed.
use crate::shared_audio_buffer::mr_shared_audio_buffer_required_bytes;
use std::{
    ffi::c_void,
    mem::size_of,
    sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering},
};

pub const VERSION: u32 = 7;
mod timestamps;
const HEADER_BYTES: u32 = 176;
const _: () = assert!(size_of::<ApplicationClock>() == HEADER_BYTES as usize);
const MAGIC: u64 = 0x4d52_4150_5043_4c4b;
pub const FRAME_MASK: u64 = 0x0000_ffff_ffff_ffff;

#[repr(C)]
pub struct ApplicationClock {
    magic: u64,
    version: u32,
    header_bytes: u32,
    pub format_word: AtomicU64,
    pub acknowledged_format: AtomicU64,
    pub consumer_position: AtomicU64,
    pub consumer_ticks: AtomicU64,
    pub consumer_running: AtomicU32,
    pub native_sample_rate: AtomicU32,
    pub consumer_sequence: AtomicU64,
    pub producer_guard: AtomicU32,
    pub native_frames: AtomicU64,
    pub native_running: AtomicU32,
    pub clock_master: AtomicU32,
    pub producer_lease: AtomicU64,
    pub target_backlog_frames: AtomicU32,
    /// Epoch-scoped pump measurements, in graph frames.
    pub producer_burst_frames: AtomicU32,
    pub converter_burst_frames: AtomicU32,
    pub publication_drought_frames: AtomicU32,
    pub starvation_episodes: AtomicU64,
    pub starvation_frames: AtomicU64,
    pub rebuffer_frames: AtomicU64,
    pub overwrite_skips: AtomicU64,
    pub consumer_burst_frames: AtomicU32,
    pub insufficient_headroom: AtomicU32,
    /// Producer-normalized source samples awaiting one graph-side report.
    pub source_sample_faults: mixrelay_audio_io::pcm::SampleFaultCounter,
    /// Nanoseconds from a time-addressed consumer's requested time to the timestamp of
    /// its next unread frame, or `NO_PHASE`. Published with the consumer position.
    pub consumer_phase: AtomicI32,
    /// How the consumer's published plan uses this source, as `ConsumerRoute` bits.
    pub consumer_route: AtomicU32,
    /// That consumer's measured nanoseconds per graph frame as `f64` bits, or 0 for a
    /// consumer that reads in sequence. Published with the consumer position.
    pub consumer_period: AtomicU64,
}

/// `consumer_phase` of a consumer without a continuous time-addressed read.
pub const NO_PHASE: i32 = i32::MIN;

/// How the consumer's active graph plan uses a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsumerRoute {
    /// No plan has published a route since the source was created.
    Unknown,
    /// The plan does not carry the source to an output, or the node is off.
    Unrouted,
    /// The plan carries the source. `head` marks the source the plan paces from its
    /// first credit, which consumes PCM retained before admission.
    Routed { head: bool },
}
const ROUTE_PUBLISHED: u32 = 1;
const ROUTE_ROUTED: u32 = 2;
const ROUTE_HEAD: u32 = 4;
impl ConsumerRoute {
    #[must_use]
    pub const fn word(self) -> u32 {
        match self {
            Self::Unknown => 0,
            Self::Unrouted => ROUTE_PUBLISHED,
            Self::Routed { head: false } => ROUTE_PUBLISHED | ROUTE_ROUTED,
            Self::Routed { head: true } => ROUTE_PUBLISHED | ROUTE_ROUTED | ROUTE_HEAD,
        }
    }
    #[must_use]
    pub const fn from_word(word: u32) -> Self {
        if word & ROUTE_PUBLISHED == 0 {
            Self::Unknown
        } else if word & ROUTE_ROUTED == 0 {
            Self::Unrouted
        } else {
            Self::Routed {
                head: word & ROUTE_HEAD != 0,
            }
        }
    }
}

fn offset(capacity: u32, channels: u32) -> Option<usize> {
    let bytes = mr_shared_audio_buffer_required_bytes(capacity, channels);
    (bytes != 0)
        .then_some(bytes)?
        .checked_add(63)
        .map(|value| value & !63)
}

#[unsafe(no_mangle)]
pub extern "C" fn mr_application_clock_required_bytes(capacity: u32, channels: u32) -> usize {
    offset(capacity, channels)
        .and_then(|value| value.checked_add(size_of::<ApplicationClock>()))
        .and_then(|value| value.checked_add(capacity as usize * 16))
        .unwrap_or(0)
}

/// # Safety
/// Memory is a live mapping of `size` bytes; its immutable geometry is not mutated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_application_clock_attach(
    memory: *mut c_void,
    size: usize,
    capacity: u32,
    channels: u32,
) -> *mut ApplicationClock {
    let Some(offset) = offset(capacity, channels) else {
        return std::ptr::null_mut();
    };
    if memory.is_null()
        || size < offset
        || size < mr_application_clock_required_bytes(capacity, channels)
        || (memory as usize)
            .checked_add(offset)
            .is_none_or(|address| address % 64 != 0)
    {
        return std::ptr::null_mut();
    }
    // The checks above establish a 64-byte-aligned address inside this mapping.
    let clock = unsafe { memory.byte_add(offset).cast::<ApplicationClock>() };
    if unsafe {
        (*clock).magic != MAGIC
            || (*clock).version != VERSION
            || usize::try_from((*clock).header_bytes).ok() != Some(size_of::<ApplicationClock>())
    } {
        return std::ptr::null_mut();
    }
    clock
}

/// # Safety
/// Memory is exclusively owned, writable, and `size` bytes long until publication.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_application_clock_initialize(
    memory: *mut c_void,
    size: usize,
    capacity: u32,
    channels: u32,
) -> *mut ApplicationClock {
    let required = mr_application_clock_required_bytes(capacity, channels);
    let Some(offset) = offset(capacity, channels) else {
        return std::ptr::null_mut();
    };
    if memory.is_null()
        || required == 0
        || size < required
        || (memory as usize)
            .checked_add(offset)
            .is_none_or(|address| address % 64 != 0)
    {
        return std::ptr::null_mut();
    }
    // The checks above establish aligned, writable storage for the whole clock.
    let clock = unsafe { memory.byte_add(offset).cast::<ApplicationClock>() };
    unsafe {
        clock.write(ApplicationClock {
            magic: MAGIC,
            version: VERSION,
            header_bytes: HEADER_BYTES,
            format_word: AtomicU64::new(0),
            acknowledged_format: AtomicU64::new(0),
            consumer_position: AtomicU64::new(0),
            consumer_ticks: AtomicU64::new(0),
            consumer_running: AtomicU32::new(0),
            native_sample_rate: AtomicU32::new(0),
            consumer_sequence: AtomicU64::new(0),
            producer_guard: AtomicU32::new(0),
            native_frames: AtomicU64::new(0),
            native_running: AtomicU32::new(0),
            clock_master: AtomicU32::new(0),
            producer_lease: AtomicU64::new(0),
            target_backlog_frames: AtomicU32::new(0),
            producer_burst_frames: AtomicU32::new(0),
            converter_burst_frames: AtomicU32::new(0),
            publication_drought_frames: AtomicU32::new(0),
            starvation_episodes: AtomicU64::new(0),
            starvation_frames: AtomicU64::new(0),
            rebuffer_frames: AtomicU64::new(0),
            overwrite_skips: AtomicU64::new(0),
            consumer_burst_frames: AtomicU32::new(0),
            insufficient_headroom: AtomicU32::new(0),
            source_sample_faults: mixrelay_audio_io::pcm::SampleFaultCounter::default(),
            consumer_phase: AtomicI32::new(NO_PHASE),
            consumer_route: AtomicU32::new(0),
            consumer_period: AtomicU64::new(0),
        });
        // SAFETY: required_bytes includes the prepared per-frame atomic sidecar.
        timestamps::initialize(clock, capacity);
    }
    clock
}

/// # Safety
/// Clock is null or points to a live initialized application clock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_application_clock_try_producer(clock: *mut ApplicationClock) -> bool {
    unsafe { clock.as_ref() }.is_some_and(|clock| {
        clock
            .producer_guard
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    })
}

/// # Safety
/// Caller owns the producer claim on this live clock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_application_clock_release_producer(clock: *mut ApplicationClock) {
    if let Some(clock) = unsafe { clock.as_ref() } {
        clock.producer_guard.store(0, Ordering::Release);
    }
}

/// # Safety
/// Clock is null or live. The caller is the single claimed consumer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_application_clock_request_rate(
    clock: *mut ApplicationClock,
    rate: u32,
) -> u64 {
    let Some(clock) = (unsafe { clock.as_ref() }) else {
        return 0;
    };
    if !(8000..=192_000).contains(&rate) {
        return 0;
    }
    let current = clock.format_word.load(Ordering::Acquire);
    if current & u64::from(u32::MAX) == u64::from(rate) {
        return current;
    }
    let generation = (current >> 32) + 1;
    // A format identity is never reused. The registry replaces this mapping
    // when its producer next attaches after terminal format generation.
    if generation > u64::from(u32::MAX) {
        return 0;
    }
    let next = (generation << 32) | u64::from(rate);
    clock.consumer_running.store(0, Ordering::Release);
    clock.format_word.store(next, Ordering::Release);
    next
}

/// # Safety
/// Clock is null or live. The caller is the single claimed consumer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_application_clock_publish_consumer(
    clock: *mut ApplicationClock,
    epoch: u64,
    next_frame: u64,
    frames: u32,
    running: bool,
    target_backlog_frames: u32,
) {
    if let Some(clock) = unsafe { clock.as_ref() } {
        clock.publish_consumer(ConsumerPublication {
            epoch,
            next_frame,
            frames,
            running,
            target_backlog_frames,
            phase_ns: NO_PHASE,
            period_ns: 0.0,
        });
    }
}

/// One consumer read: its position and progress, and for a reader that follows a
/// clock by time, where that clock stands against the published frame timestamps.
#[derive(Clone, Copy, Debug)]
pub struct ConsumerPublication {
    pub epoch: u64,
    pub next_frame: u64,
    pub frames: u32,
    pub running: bool,
    pub target_backlog_frames: u32,
    pub phase_ns: i32,
    pub period_ns: f64,
}

impl ApplicationClock {
    /// Publishes one consumer read under the consumer sequence. The caller is the
    /// single claimed consumer.
    pub fn publish_consumer(&self, publication: ConsumerPublication) {
        self.consumer_sequence.fetch_add(1, Ordering::AcqRel);
        self.consumer_position.store(
            (publication.epoch << 48) | (publication.next_frame & FRAME_MASK),
            Ordering::Release,
        );
        self.consumer_ticks
            .fetch_add(u64::from(publication.frames), Ordering::Release);
        self.consumer_running
            .store(u32::from(publication.running), Ordering::Release);
        self.target_backlog_frames
            .store(publication.target_backlog_frames, Ordering::Release);
        self.consumer_phase
            .store(publication.phase_ns, Ordering::Release);
        self.consumer_period
            .store(publication.period_ns.to_bits(), Ordering::Release);
        self.consumer_sequence.fetch_add(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{Layout, alloc_zeroed, dealloc};

    struct Storage(*mut c_void, Layout);
    impl Storage {
        fn new() -> Self {
            let layout =
                Layout::from_size_align(mr_application_clock_required_bytes(1024, 2), 64).unwrap();
            let memory = unsafe { alloc_zeroed(layout) }.cast::<c_void>();
            assert!(!memory.is_null());
            Self(memory, layout)
        }
        fn clock(&self) -> *mut ApplicationClock {
            unsafe { mr_application_clock_initialize(self.0, self.1.size(), 1024, 2) }
        }
    }
    impl Drop for Storage {
        fn drop(&mut self) {
            unsafe { dealloc(self.0.cast(), self.1) };
        }
    }

    #[test]
    fn current_layout_rejects_retired_versions_and_invalid_geometry_without_touching_pcm() {
        assert_eq!(size_of::<ApplicationClock>(), 176);
        assert_eq!(
            std::mem::offset_of!(ApplicationClock, target_backlog_frames),
            96
        );
        assert_eq!(mr_application_clock_required_bytes(0, 2), 0);
        let storage = Storage::new();
        let clock = storage.clock();
        assert!(!clock.is_null());
        unsafe {
            assert_eq!(
                mr_application_clock_attach(storage.0, storage.1.size(), 1024, 2),
                clock
            );
            (*clock).version = 2;
            assert!(mr_application_clock_attach(storage.0, storage.1.size(), 1024, 2).is_null());
            (*clock).version = VERSION;
            (*clock).header_bytes = 96;
            assert!(mr_application_clock_attach(storage.0, storage.1.size(), 1024, 2).is_null());
            (*clock).header_bytes = u32::try_from(size_of::<ApplicationClock>()).unwrap();
            (*clock).magic = 0;
            assert!(mr_application_clock_attach(storage.0, storage.1.size(), 1024, 2).is_null());
            for (memory, bytes, capacity) in [
                (std::ptr::null_mut(), storage.1.size(), 1024),
                (storage.0, 1, 1024),
                (storage.0, storage.1.size() - 1, 1024),
                (storage.0.byte_add(1), storage.1.size(), 1024),
                (storage.0, storage.1.size(), 0),
            ] {
                assert!(mr_application_clock_attach(memory, bytes, capacity, 2).is_null());
                assert!(mr_application_clock_initialize(memory, bytes, capacity, 2).is_null());
            }
        }
    }

    #[test]
    fn bounded_publication_is_allocation_free_and_carries_target_with_cursor() {
        let storage = Storage::new();
        let pointer = storage.clock();
        let clock = unsafe { &*pointer };
        let measured = allocation_counter::measure(|| unsafe {
            assert!(!mr_application_clock_try_producer(std::ptr::null_mut()));
            mr_application_clock_release_producer(std::ptr::null_mut());
            assert!(mr_application_clock_try_producer(pointer));
            assert!(!mr_application_clock_try_producer(pointer));
            mr_application_clock_release_producer(pointer);
            assert_eq!(
                mr_application_clock_request_rate(std::ptr::null_mut(), 48000),
                0
            );
            assert_eq!(mr_application_clock_request_rate(pointer, 1), 0);
            let format = mr_application_clock_request_rate(pointer, 48000);
            assert_eq!(format, mr_application_clock_request_rate(pointer, 48000));
            mr_application_clock_publish_consumer(std::ptr::null_mut(), 1, 0, 128, false, 512);
            for (next, target, running) in [(128, 384, true), (128, 640, false), (256, 640, true)] {
                mr_application_clock_publish_consumer(pointer, 3, next, 128, running, target);
                assert_eq!(
                    clock.consumer_position.load(Ordering::Acquire),
                    (3 << 48) | next
                );
                assert_eq!(clock.target_backlog_frames.load(Ordering::Acquire), target);
                assert_eq!(
                    clock.consumer_running.load(Ordering::Acquire),
                    u32::from(running)
                );
                assert_eq!(clock.consumer_sequence.load(Ordering::Acquire) & 1, 0);
            }
            assert_eq!(clock.consumer_ticks.load(Ordering::Acquire), 384);
            clock.format_word.store(u64::MAX, Ordering::Release);
            assert_eq!(mr_application_clock_request_rate(pointer, 48000), 0);
        });
        assert_eq!(measured.count_total, 0);
    }
}
