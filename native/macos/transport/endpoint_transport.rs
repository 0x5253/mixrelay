//! Caller-owned duplex mappings and native callback credits; no process pointers
//! are stored in shared memory. Geometry is immutable after initialization.

use crate::shared_audio_buffer::{self as audio, SharedAudioBuffer};
use std::{
    ffi::c_void,
    mem::{align_of, size_of},
    ptr,
    sync::atomic::{AtomicU32, AtomicU64, Ordering, fence},
};

const MAGIC: u64 = 0x4d52_4550_4f49_4f31;
const VERSION: u32 = 3;
const CLOCK_MAGIC: u64 = 0x4d52_434c_4f43_4b32;
const FRAME_MASK: u64 = (1 << 48) - 1;
const MAX_CLOCK_TAG: u64 = (1 << 16) - 1;
const TERMINAL: u64 = 1 << 63;
const UNREADY: u64 = 1 << 62;

mod demand;
use demand::Demand;
// The model-test target instruments this same algorithm with Loom atomics.
use std::sync::atomic as demand_atomics;

unsafe extern "C" {
    fn arc4random_buf(buffer: *mut c_void, length: usize);
}
// Darwin's max_align_t is long double: 16-byte aligned on Intel, 8 on ARM.
const STORAGE_ALIGNMENT: usize = if cfg!(target_arch = "x86_64") { 16 } else { 8 };

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct EndpointTransportConfig {
    pub endpoint_id: [u8; 16],
    pub capacity_frames: u32,
    pub render_channels: u32,
    pub capture_channels: u32,
}

#[repr(C)]
pub struct EndpointTransport {
    magic: u64,
    version: u32,
    total_bytes: u32,
    endpoint_id: [u8; 16],
    capacity_frames: u32,
    render_channels: u32,
    capture_channels: u32,
    render_offset: u32,
    capture_offset: u32,
    reserved: u32,
    // Formerly heartbeats; nothing reads liveness from the mapping, so the HAL IO path
    // writes nothing here. Kept so the header layout and size are unchanged.
    reserved_tail: [u64; 2],
}

#[repr(C)]
struct EndpointClock {
    magic: u64,
    generation: AtomicU64,
    render: Demand,
    capture: Demand,
    sample_rate: AtomicU32,
    reserved: u32,
    // Preserve the full HAL lifecycle identity; stamp tags are allocated within
    // this mapping, not derived by truncating/modulo-reducing this identity.
    native_epoch: AtomicU64,
    // Immutable mapping identity survives FD duplication and is independent of
    // process restarts and POSIX shared-memory fstat device/inode placeholders.
    incarnation: [u8; 16],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
/// Coherent native callback progress for one mapping-local clock generation.
/// A callback quantum describes the latest accepted publication, not the
/// accumulated credit since a consumer's previous observation. Contention can
/// defer publication; an unchanged value does not prove the device stopped.
pub struct EndpointClockSnapshot {
    /// Mapping-local generation; zero means no committed clock state.
    pub epoch: u64,
    /// Exclusive end frame of the latest render callback in this generation.
    /// Zero means no render demand has been published in the current generation.
    pub render_through: u64,
    /// Exclusive end frame requested by the latest capture callback; zero means
    /// no capture demand has been published in the current generation.
    pub capture_through: u64,
    /// Device-owned nominal sample rate in Hz; zero means unavailable.
    pub sample_rate: u32,
    /// One for a running committed clock, zero for a stopped committed clock.
    /// An unready clock makes snapshot acquisition fail; it is not a stopped snapshot.
    pub running: u32,
    /// Sample frames requested by the capture callback paired with
    /// `capture_through`; zero means no publication in this generation.
    pub capture_frames: u32,
    /// Sample frames provided by the render callback paired with
    /// `render_through`; zero means no publication in this generation.
    pub render_frames: u32,
}

const _: () = assert!(size_of::<EndpointTransport>() == 72);
const _: () = assert!(std::mem::offset_of!(EndpointTransport, reserved) == 52);
const _: () = assert!(size_of::<EndpointClock>() == 96);
const _: () = assert!(size_of::<EndpointClockSnapshot>() == 40);
const _: () = assert!(std::mem::offset_of!(EndpointClockSnapshot, capture_frames) == 32);
const _: () = assert!(std::mem::offset_of!(EndpointClockSnapshot, render_frames) == 36);

struct StorageLayout {
    audio_bytes: u32,
    render_offset: u32,
    capture_offset: u32,
}

fn align_up(value: usize) -> Option<usize> {
    value
        .checked_add(STORAGE_ALIGNMENT - 1)
        .map(|value| value & !(STORAGE_ALIGNMENT - 1))
}

fn storage_layout(config: &EndpointTransportConfig) -> Option<StorageLayout> {
    if config.endpoint_id == [0; 16]
        || config.capacity_frames == 0
        || (config.render_channels == 0 && config.capture_channels == 0)
    {
        return None;
    }
    let mut cursor = align_up(size_of::<EndpointTransport>())?;
    let mut offsets = [0; 2];
    for (index, channels) in [config.render_channels, config.capture_channels]
        .into_iter()
        .enumerate()
    {
        if channels == 0 {
            continue;
        }
        let bytes = audio::mr_shared_audio_buffer_required_bytes(config.capacity_frames, channels);
        if bytes == 0 {
            return None;
        }
        offsets[index] = u32::try_from(cursor).ok()?;
        cursor = align_up(cursor.checked_add(bytes)?)?;
    }
    Some(StorageLayout {
        audio_bytes: u32::try_from(cursor).ok()?,
        render_offset: offsets[0],
        capture_offset: offsets[1],
    })
}

impl EndpointTransport {
    fn config(&self) -> EndpointTransportConfig {
        EndpointTransportConfig {
            endpoint_id: self.endpoint_id,
            capacity_frames: self.capacity_frames,
            render_channels: self.render_channels,
            capture_channels: self.capture_channels,
        }
    }
    unsafe fn clock(&self) -> Option<&EndpointClock> {
        if self.reserved == 0 {
            return None;
        }
        // SAFETY: initialize/attach validated the extension's offset, alignment,
        // size and magic. The caller retains the mapping throughout this borrow.
        let pointer = unsafe {
            ptr::from_ref(self)
                .cast::<u64>()
                .add(self.reserved as usize / size_of::<u64>())
                .cast::<EndpointClock>()
        };
        Some(unsafe { &*pointer })
    }
    unsafe fn buffer(&self, offset: u32) -> *mut SharedAudioBuffer {
        if offset == 0 {
            return ptr::null_mut();
        }
        // SAFETY: offsets and total length are immutable validated geometry.
        let memory = unsafe {
            ptr::from_ref(self)
                .cast::<u8>()
                .add(offset as usize)
                .cast_mut()
                .cast()
        };
        unsafe {
            audio::mr_shared_audio_buffer_attach(memory, (self.total_bytes - offset) as usize)
        }
    }
}

/// # Safety
/// A non-null configuration must be readable for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_endpoint_transport_required_bytes(
    config: *const EndpointTransportConfig,
) -> usize {
    unsafe { config.as_ref() }
        .and_then(storage_layout)
        .map_or(0, |layout| {
            layout.audio_bytes as usize + size_of::<EndpointClock>()
        })
}

/// # Safety
/// `memory` must exclusively own its stated writable extent; configuration must
/// be readable and disjoint from that storage. No existing attached users exist.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_endpoint_transport_initialize(
    memory: *mut c_void,
    memory_size: usize,
    config: *const EndpointTransportConfig,
) -> *mut EndpointTransport {
    let Some(config) = (unsafe { config.as_ref() }) else {
        return ptr::null_mut();
    };
    let Some(layout) = storage_layout(config) else {
        return ptr::null_mut();
    };
    let required = layout.audio_bytes as usize + size_of::<EndpointClock>();
    if memory.is_null()
        || !(memory as usize).is_multiple_of(align_of::<EndpointTransport>())
        || memory_size < required
    {
        return ptr::null_mut();
    }
    let mut incarnation = [0; 16];
    // SAFETY: this control-only initializer owns all 16 stack bytes. OS entropy
    // does not run on callbacks. Reject an all-zero result without retrying.
    unsafe {
        arc4random_buf(incarnation.as_mut_ptr().cast(), incarnation.len());
    }
    if incarnation == [0; 16] {
        return ptr::null_mut();
    }
    let pointer = memory.cast::<EndpointTransport>();
    // SAFETY: exclusive initialized geometry and all trailing subregions fit the
    // caller extent. Every interior offset is at least 8-byte aligned.
    unsafe {
        ptr::write_bytes(memory.cast::<u8>(), 0, required);
        pointer.write(EndpointTransport {
            magic: MAGIC,
            version: VERSION,
            total_bytes: layout.audio_bytes,
            endpoint_id: config.endpoint_id,
            capacity_frames: config.capacity_frames,
            render_channels: config.render_channels,
            capture_channels: config.capture_channels,
            render_offset: layout.render_offset,
            capture_offset: layout.capture_offset,
            reserved: layout.audio_bytes,
            reserved_tail: [0; 2],
        });
        let clock = memory
            .cast::<u64>()
            .add(layout.audio_bytes as usize / size_of::<u64>())
            .cast::<EndpointClock>();
        clock.write(EndpointClock {
            magic: CLOCK_MAGIC,
            generation: AtomicU64::new(0),
            render: Demand::new(),
            capture: Demand::new(),
            sample_rate: AtomicU32::new(0),
            reserved: 0,
            native_epoch: AtomicU64::new(0),
            incarnation,
        });
        for (offset, channels) in [
            (layout.render_offset, config.render_channels),
            (layout.capture_offset, config.capture_channels),
        ] {
            if offset != 0 {
                let initialized = audio::mr_shared_audio_buffer_initialize(
                    memory.cast::<u8>().add(offset as usize).cast(),
                    (layout.audio_bytes - offset) as usize,
                    config.capacity_frames,
                    channels,
                );
                if initialized.is_null() {
                    return ptr::null_mut();
                }
            }
        }
    }
    pointer
}

/// # Safety
/// The initialized mapping must remain accessible; its geometry must not change.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_endpoint_transport_attach(
    memory: *mut c_void,
    memory_size: usize,
) -> *mut EndpointTransport {
    if memory.is_null()
        || !(memory as usize).is_multiple_of(align_of::<EndpointTransport>())
        || memory_size < size_of::<EndpointTransport>()
    {
        return ptr::null_mut();
    }
    let pointer = memory.cast::<EndpointTransport>();
    let transport = unsafe { &*pointer };
    let Some(layout) = storage_layout(&transport.config()) else {
        return ptr::null_mut();
    };
    if transport.magic != MAGIC
        || transport.version != VERSION
        || transport.total_bytes != layout.audio_bytes
        || transport.render_offset != layout.render_offset
        || transport.capture_offset != layout.capture_offset
        || memory_size < layout.audio_bytes as usize
    {
        return ptr::null_mut();
    }
    if transport.reserved != layout.audio_bytes
        || memory_size - (layout.audio_bytes as usize) < size_of::<EndpointClock>()
    {
        return ptr::null_mut();
    }
    if unsafe { transport.clock() }
        .is_none_or(|clock| clock.magic != CLOCK_MAGIC || clock.incarnation == [0; 16])
    {
        return ptr::null_mut();
    }
    for offset in [layout.render_offset, layout.capture_offset] {
        if offset != 0 && unsafe { transport.buffer(offset) }.is_null() {
            return ptr::null_mut();
        }
    }
    pointer
}

/// Hides the clock before any associated PCM epoch is invalidated or reset.
/// Remote observers must defer their checked PCM operations until final commit.
/// Re-entering an in-progress transition is harmless for its one control owner.
/// # Safety
/// Transport is attached and live. One control writer owns the transition and
/// has drained local callbacks before changing their ring epochs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_endpoint_transport_begin_clock_transition(
    transport: *mut EndpointTransport,
) -> bool {
    let Some(transport) = (unsafe { transport.as_ref() }) else {
        return false;
    };
    let Some(clock) = (unsafe { transport.clock() }) else {
        return false;
    };
    let previous = clock.generation.fetch_or(UNREADY, Ordering::AcqRel);
    // Subsequent ring publication is ordered after invalidating the clock, so
    // a snapshot observing new PCM epochs cannot retain the preceding clock.
    fence(Ordering::Release);
    previous & TERMINAL == 0
}

/// Commits the ready lifecycle after the transition's final PCM invalidation.
/// # Safety
/// Non-null transport must be attached and live; one control writer owns state
/// changes. A transition must already be active, local callbacks drained, and
/// final capture rearm published. A missing transition never publishes state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_endpoint_transport_clock_state(
    transport: *mut EndpointTransport,
    epoch: u64,
    sample_rate: u32,
    running: bool,
) {
    let Some(transport) = (unsafe { transport.as_ref() }) else {
        return;
    };
    let Some(clock) = (unsafe { transport.clock() }) else {
        return;
    };
    let previous = clock.generation.load(Ordering::Acquire);
    if previous & TERMINAL != 0 || previous & UNREADY == 0 {
        return;
    }
    let previous_tag = (previous & !UNREADY) >> 1;
    if previous_tag >= MAX_CLOCK_TAG {
        // Remote snapshots cannot be drained here. Retire the entire mapping
        // rather than making an ancient reader's generation valid again.
        clock.generation.store(TERMINAL, Ordering::Release);
        return;
    }
    clock.render.reset();
    clock.capture.reset();
    clock.sample_rate.store(sample_rate, Ordering::Relaxed);
    clock.native_epoch.store(epoch, Ordering::Relaxed);
    let generation = ((previous_tag + 1) << 1) | u64::from(running);
    clock.generation.store(generation, Ordering::Release);
}

/// Coalesces absolute callback credits with one bounded compare/exchange.
/// # Safety
/// Non-null transport must be attached and live. Lifecycle replacement requires
/// local demand writers to be quiescent; remote snapshots may continue.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_endpoint_transport_clock_demand(
    transport: *mut EndpointTransport,
    render: bool,
    through_frame: u64,
    frames: u32,
) {
    let Some(transport) = (unsafe { transport.as_ref() }) else {
        return;
    };
    let Some(clock) = (unsafe { transport.clock() }) else {
        return;
    };
    if through_frame == 0 || through_frame > FRAME_MASK || frames == 0 {
        return;
    }
    let generation = clock.generation.load(Ordering::Acquire);
    if generation & (TERMINAL | UNREADY) != 0 || generation & 1 == 0 {
        return;
    }
    let next = ((generation >> 1) << 48) | through_frame;
    if !render {
        clock.capture.publish(next, frames);
        return;
    }
    clock.render.publish(next, frames);
}

/// # Safety
/// Non-null transport must be attached and live; snapshot must be writable and
/// disjoint from shared storage and other concurrent output users.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_endpoint_transport_clock_snapshot(
    transport: *const EndpointTransport,
    snapshot: *mut EndpointClockSnapshot,
) -> bool {
    if snapshot.is_null() {
        return false;
    }
    let Some(transport) = (unsafe { transport.as_ref() }) else {
        return false;
    };
    let Some(clock) = (unsafe { transport.clock() }) else {
        return false;
    };
    let generation = clock.generation.load(Ordering::Acquire);
    if generation == 0 || generation & (TERMINAL | UNREADY) != 0 {
        return false;
    }
    let Some((render, render_frames)) = clock.render.read() else {
        return false;
    };
    let Some((capture, capture_frames)) = clock.capture.read() else {
        return false;
    };
    let epoch = generation >> 1;
    let sample_rate = clock.sample_rate.load(Ordering::Relaxed);
    fence(Ordering::Acquire);
    if generation != clock.generation.load(Ordering::Acquire) {
        return false;
    }
    unsafe {
        snapshot.write(EndpointClockSnapshot {
            epoch,
            render_through: if render >> 48 == epoch {
                render & FRAME_MASK
            } else {
                0
            },
            capture_through: if capture >> 48 == epoch {
                capture & FRAME_MASK
            } else {
                0
            },
            sample_rate,
            running: u32::from(generation & 1 != 0),
            capture_frames: if capture >> 48 == epoch {
                capture_frames
            } else {
                0
            },
            render_frames: if render >> 48 == epoch {
                render_frames
            } else {
                0
            },
        });
    }
    true
}

/// Validates only lifecycle identity, not the concurrently advancing callback deadline.
/// PCM already has absolute coordinates; an ordinary deadline publication must not
/// invalidate its still-live mapping, epoch, rate or running state.
///
/// # Safety
/// Non-null transport must be attached and its complete mapping must remain pinned
/// through this call. `expected` must not alias memory concurrently written by others.
#[must_use]
pub unsafe fn clock_matches(
    transport: *const EndpointTransport,
    expected: &EndpointClockSnapshot,
) -> bool {
    // SAFETY: the caller pins the attached transport and its initialized clock storage.
    let Some(clock) = (unsafe { transport.as_ref().and_then(|value| value.clock()) }) else {
        return false;
    };
    let generation = clock.generation.load(Ordering::Acquire);
    if generation == 0
        || generation & (TERMINAL | UNREADY) != 0
        || generation >> 1 != expected.epoch
        || u32::from(generation & 1 != 0) != expected.running
        || clock.sample_rate.load(Ordering::Relaxed) != expected.sample_rate
    {
        return false;
    }
    fence(Ordering::Acquire);
    generation == clock.generation.load(Ordering::Acquire)
}

/// Signals the control owner to publish a new mapping, retaining old reader
/// owners until they detach. Never clear or reuse a terminal mapping in place.
/// # Safety
/// Non-null transport must be attached and remain live throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_endpoint_transport_requires_replacement(
    transport: *const EndpointTransport,
) -> bool {
    unsafe { transport.as_ref() }.is_some_and(|transport| {
        unsafe { transport.clock() }
            .is_some_and(|clock| clock.generation.load(Ordering::Acquire) & TERMINAL != 0)
    })
}

/// Copies the immutable 128-bit mapping identity, including across FD duplication.
/// # Safety
/// Non-null transport must be attached and live. `incarnation` must point to 16
/// writable bytes disjoint from the shared mapping and concurrent destinations.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_endpoint_transport_incarnation(
    transport: *const EndpointTransport,
    incarnation: *mut u8,
) -> bool {
    if incarnation.is_null() {
        return false;
    }
    let Some(transport) = (unsafe { transport.as_ref() }) else {
        return false;
    };
    let Some(clock) = (unsafe { transport.clock() }) else {
        return false;
    };
    // SAFETY: caller provides 16 disjoint writable bytes; identity is immutable.
    unsafe {
        ptr::copy_nonoverlapping(clock.incarnation.as_ptr(), incarnation, 16);
    }
    true
}

/// # Safety
/// Transport must be attached and live; config must be writable and disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_endpoint_transport_config(
    transport: *const EndpointTransport,
    config: *mut EndpointTransportConfig,
) -> bool {
    if config.is_null() {
        return false;
    }
    let Some(transport) = (unsafe { transport.as_ref() }) else {
        return false;
    };
    unsafe {
        config.write(transport.config());
    }
    true
}

/// # Safety
/// Non-null transport must be attached; the returned view borrows its mapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_endpoint_transport_render_buffer(
    transport: *mut EndpointTransport,
) -> *mut SharedAudioBuffer {
    unsafe { transport.as_ref() }.map_or(ptr::null_mut(), |transport| unsafe {
        transport.buffer(transport.render_offset)
    })
}
/// # Safety
/// Non-null transport must be attached; the returned view borrows its mapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_endpoint_transport_capture_buffer(
    transport: *mut EndpointTransport,
) -> *mut SharedAudioBuffer {
    unsafe { transport.as_ref() }.map_or(ptr::null_mut(), |transport| unsafe {
        transport.buffer(transport.capture_offset)
    })
}

#[cfg(test)]
#[path = "unit_tests/endpoint_transport.rs"]
mod tests;

#[cfg(test)]
#[path = "unit_tests/demand.rs"]
mod demand_tests;
