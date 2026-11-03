use super::{
    DeviceIo, RenderHook, control,
    native::{self, BufferInfo, System},
    queues,
};
use crate::callback_clock::mr_callback_clock_discontinuities;
use std::{
    ffi::{CStr, c_char, c_void},
    ptr,
    sync::atomic::Ordering::{Acquire, Relaxed, Release},
};

/// Observes the accumulated ticks counter.
/// # Safety
/// IO remains initialized and retained through this atomic observation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_ticks(io: *mut DeviceIo) -> u64 {
    // SAFETY: only independently atomic progress is read from the retained owner.
    unsafe { io.as_ref() }.map_or(0, |io| io.progress.ticks.load(Acquire))
}

/// Whether callback-observed invalidation has withdrawn this registration.
/// This is one atomic observation, without native property queries. A listener
/// notification becomes visible here when a callback observes its local latch.
/// Null owners are invalid; only drained connection preparation clears the latch.
/// # Safety
/// Non-null IO remains initialized and retained through this atomic observation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_invalid(io: *mut DeviceIo) -> bool {
    // SAFETY: only independently atomic progress is read from the retained owner.
    unsafe { io.as_ref() }.is_none_or(|io| io.progress.invalid.load(Acquire))
}

/// Observes the accumulated native frames counter.
/// # Safety
/// IO remains initialized and retained through this atomic observation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_native_frames(io: *mut DeviceIo) -> u64 {
    // SAFETY: only independently atomic progress is read from the retained owner.
    unsafe { io.as_ref() }.map_or(0, |io| io.progress.native_frames.load(Acquire))
}

/// Observes the accumulated graph frames counter.
/// # Safety
/// IO remains initialized and retained through this atomic observation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_graph_frames(io: *mut DeviceIo) -> u64 {
    // SAFETY: only independently atomic progress is read from the retained owner.
    unsafe { io.as_ref() }.map_or(0, |io| io.progress.graph_frames.load(Acquire))
}

/// The latest callback's mean-frame host ticks, or zero before any timed callback.
/// # Safety
/// IO remains initialized and retained through this atomic observation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_callback_host(io: *mut DeviceIo) -> u64 {
    // SAFETY: only independently atomic progress is read from the retained owner.
    unsafe { io.as_ref() }.map_or(0, |io| io.progress.callback_host.load(Acquire))
}

/// Transfers a complete block through the graph producer's designated ring.
/// `host` is the graph cycle's host time in ticks, shared by all of its transfers.
/// # Safety
/// IO is retained and this graph producer is serialized. Samples owns
/// frames*channels readable floats disjoint from queue storage; frames above 4096
/// are rejected before accessing PCM. The opposite ring cursor may run concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_graph_write(
    io: *mut DeviceIo,
    samples: *const f32,
    frames: u32,
    host: u64,
) -> u32 {
    // SAFETY: role-specific queue access never forms an exclusive whole-owner borrow.
    unsafe { io.as_ref() }.map_or(0, |io| unsafe {
        queues::graph_write(io, samples, frames, host)
    })
}

/// Transfers a complete block through the SRC owner consumer's designated ring.
/// # Safety
/// IO is retained and this SRC owner consumer is serialized. Samples owns
/// frames*channels writable floats disjoint from queue storage; frames above 4096
/// are rejected before accessing PCM. The opposite ring cursor may run concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_bridge_read(
    io: *mut DeviceIo,
    samples: *mut f32,
    frames: u32,
) -> u32 {
    // SAFETY: role-specific queue access never forms an exclusive whole-owner borrow.
    unsafe { io.as_ref() }.map_or(0, |io| unsafe { queues::bridge_read(io, samples, frames) })
}

/// Transfers a complete block through the SRC owner producer's designated ring.
/// # Safety
/// IO is retained and this SRC owner producer is serialized. Samples owns
/// frames*channels readable floats disjoint from queue storage; frames above 4096
/// are rejected before accessing PCM. The opposite ring cursor may run concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_bridge_write(
    io: *mut DeviceIo,
    samples: *const f32,
    frames: u32,
) -> u32 {
    // SAFETY: role-specific queue access never forms an exclusive whole-owner borrow.
    unsafe { io.as_ref() }.map_or(0, |io| unsafe { queues::bridge_write(io, samples, frames) })
}

/// Allocates a complete disconnected device owner. Invalid geometry or allocation
/// failure returns null without retaining resources.
/// # Safety
/// UID is a readable NUL-terminated string for this call; it is copied into the owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_create(
    uid: *const c_char,
    input: bool,
    channels: u32,
    rate: u32,
) -> *mut DeviceIo {
    if uid.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: API requires a complete immutable NUL-terminated identifier.
    DeviceIo::new(
        unsafe { CStr::from_ptr(uid) },
        input,
        channels,
        rate,
        mixrelay_realtime::Timebase::current(),
    )
    .map_or(ptr::null_mut(), DeviceIo::publish)
}

/// Creates a device whose aggregate owner is retained externally by the engine.
/// # Safety
/// UID satisfies create's contract. Non-null group is the matching live aggregate
/// owner and remains retained until this device's acknowledged destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_create_group(
    group: *mut c_void,
    uid: *const c_char,
    input: bool,
    channels: u32,
    rate: u32,
) -> *mut DeviceIo {
    // SAFETY: delegates the same immutable UID contract to creation.
    let io = unsafe { mr_device_io_create(uid, input, channels, rate) };
    // SAFETY: unpublished new owner has no concurrent accesses or native registration.
    if let Some(owner) = unsafe { io.as_mut() } {
        owner.selection.group = group;
    }
    io
}

/// Registers and starts once; zero indicates success, negative preflight or native
/// status indicates failure. Partial handles remain owned for later teardown.
/// # Safety
/// IO is retained; the sole engine control worker excludes other control calls.
/// Prior registration must have drained before prepare touches callback-local state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_connect_status(io: *mut DeviceIo, expected: u32) -> i32 {
    // SAFETY: stable context retained by its one control owner; only control cell mutates.
    unsafe { io.as_ref() }.map_or(-2, |io| unsafe { control::connect(io, expected, &System) })
}

/// Connects without an expected-device constraint; false retains all owned resources.
/// # Safety
/// Same retained sole-control-owner contract as `connect_status`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_connect(io: *mut DeviceIo) -> bool {
    // SAFETY: exact forwarding of the control ownership contract.
    unsafe { mr_device_io_connect_status(io, 0) == 0 }
}

/// Attempts stop/unregister once. False leaves the entire callback context retained.
/// # Safety
/// Sole control owner retains IO and excludes connect/disconnect/destruction calls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_disconnect(io: *mut DeviceIo) -> bool {
    // SAFETY: callbacks touch only disjoint callback cells/queues/atomics.
    unsafe { io.as_ref() }.is_none_or(|io| unsafe { control::disconnect(io, &System) })
}

/// Releases the allocation only after unregister confirms drain. False retains
/// the exact pointer and every queue/scratch/UID resource for retry. Null succeeds.
/// # Safety
/// Caller is the sole allocation owner and has stopped worker/graph readers.
/// On success no pointer or reference may be used again; callbacks were drained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_destroy(io: *mut DeviceIo) -> bool {
    // SAFETY: the outer allocation owner has stopped graph/worker roles.
    unsafe { destroy_with(io, |io| control::disconnect(io, &System)) }
}

// SAFETY: exclusive allocation owner, stopped graph/worker; disconnect must return
// true only after the retained callback registration acknowledges native drain.
pub(super) unsafe fn destroy_with(
    io: *mut DeviceIo,
    disconnect: impl FnOnce(&DeviceIo) -> bool,
) -> bool {
    if io.is_null() {
        return true;
    }
    // SAFETY: caller owns the stable allocation, retained through disconnect failure.
    if !disconnect(unsafe { &*io }) {
        return false;
    }
    // SAFETY: successful unregister acknowledges callback drain and caller stopped
    // graph/worker roles. Allocation originated from the global allocator layout.
    drop(unsafe { Box::from_raw(io) });
    true
}

/// Installs the engine's retained render slot before native registration.
/// # Safety
/// IO has no registered callback, and control excludes all other mutation. Context
/// and hook remain valid until IO teardown; hook never retains native buffers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_render_hook(
    io: *mut DeviceIo,
    context: *mut c_void,
    hook: Option<RenderHook>,
) {
    // SAFETY: owner retained; only its unpublished callback cell is mutated.
    if let Some(io) = unsafe { io.as_ref() } {
        // SAFETY: no callback is registered while the render slot is installed.
        let callback = unsafe { &mut *io.callback.get() };
        callback.hook = hook;
        callback.context = context;
    }
}

/// Resolves immutable device selection without touching the worker's mutable connection.
/// # Safety
/// IO retained; each output is exclusively writable u32 storage disjoint from IO.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_direct_binding(
    io: *mut DeviceIo,
    device: *mut u32,
    offset: *mut u32,
    total: *mut u32,
) -> bool {
    if device.is_null() || offset.is_null() || total.is_null() {
        return false;
    }
    // SAFETY: immutable selection remains live while the engine retains IO.
    let Some(io) = (unsafe { io.as_ref() }) else {
        return false;
    };
    let Some(binding) = control::direct_binding(io, &System) else {
        return false;
    };
    // SAFETY: caller supplies three disjoint exclusive scalar outputs.
    unsafe {
        device.write(binding.device);
        offset.write(binding.offset);
        total.write(binding.total);
    }
    true
}

/// Observes current native liveness, format and selected default identity.
/// # Safety
/// The single control owner excludes connection state changes during this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_alive(io: *mut DeviceIo) -> bool {
    // SAFETY: control-only query never touches callback scratch or queue cursors.
    unsafe { io.as_ref() }.is_some_and(|io| unsafe { control::alive(io, &System) })
}

/// Returns connected stream rate, observed without changing hardware settings.
/// # Safety
/// Sole control owner excludes connection writes; IO remains retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_native_rate(io: *mut DeviceIo) -> u32 {
    // SAFETY: callback does not access Control, and control calls are serialized.
    unsafe { io.as_ref() }.map_or(0, |io| unsafe { (*io.control.get()).connection.rate })
}

/// Returns one proven native clock object, or zero for absent/unknown/user aggregates.
/// # Safety
/// Sole control owner excludes connection writes; aggregate owner remains retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_clock_device(io: *mut DeviceIo) -> u32 {
    // SAFETY: serialized control reads only immutable selection and connection cell.
    unsafe { io.as_ref() }.map_or(0, |io| unsafe { control::clock_device(io, &System) })
}

/// Reads scalar buffer facts using immutable selection; false leaves a non-null output zeroed.
/// # Safety
/// IO is null or retained, and info is null or exclusively owns a complete `BufferInfo`
/// disjoint from IO.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_buffer_info(
    io: *mut DeviceIo,
    info: *mut BufferInfo,
) -> bool {
    if info.is_null() {
        return false;
    }
    // SAFETY: caller grants complete, aligned output storage. Write initializes
    // an output-only slot without first creating a reference to uninitialized data.
    unsafe { info.write(BufferInfo::default()) };
    // SAFETY: immutable selection remains live while the engine retains IO.
    let Some(io) = (unsafe { io.as_ref() }) else {
        return false;
    };
    let Some(snapshot) = control::buffer_info(io, &System) else {
        return false;
    };
    // SAFETY: the same exclusive output slot remains writable until return.
    unsafe { info.write(snapshot) };
    true
}

/// Reads native buffer frames. Missing property returns zero; no state is changed.
#[unsafe(no_mangle)]
pub extern "C" fn mr_device_buffer_size(device: u32) -> u32 {
    native::buffer_size(device, &System)
}

/// Writes one buffer-size property; the engine owns validation and readback policy.
#[unsafe(no_mangle)]
pub extern "C" fn mr_device_buffer_write(device: u32, frames: u32) -> bool {
    native::buffer_write(device, frames, &System)
}

/// Reads total queue overflow events from independent atomic observations.
/// # Safety
/// IO and both initialized rings remain retained throughout the observation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_overflows(io: *mut DeviceIo) -> u64 {
    // SAFETY: ring stats access immutable geometry and atomics only.
    unsafe { io.as_ref() }.map_or(0, |io| {
        io.native_ring
            .stats()
            .overflows
            .wrapping_add(io.graph_ring.stats().overflows)
    })
}

/// Observes queued frames in the chosen ring's native or graph clock.
/// # Safety
/// The engine retains the complete initialized IO owner until return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_queued(io: *mut DeviceIo, graph: bool) -> u32 {
    // SAFETY: immutable ring geometry and atomic diagnostics only.
    unsafe { io.as_ref() }.map_or(0, |io| io.ring(graph).stats().queued)
}

/// Counts transitions from primed PCM to shortage in the selected ring.
/// Worker-side empty polls are not device deadline misses; select the graph
/// ring for input and the native ring for output when diagnosing audible I/O.
/// # Safety
/// The engine retains the complete initialized IO owner until return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_starvations(io: *mut DeviceIo, graph: bool) -> u64 {
    // SAFETY: ring stats read immutable geometry and atomic counters only.
    unsafe { io.as_ref() }.map_or(0, |io| io.ring(graph).stats().starvations)
}

/// Observes the latest callback or graph quantum, zero before first processing.
/// # Safety
/// The engine retains IO through this atomic observation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_quantum(io: *mut DeviceIo, graph: bool) -> u32 {
    // SAFETY: only independently atomic progress is observed.
    unsafe { io.as_ref() }.map_or(0, |io| {
        if graph {
            io.progress.graph_quantum.load(Relaxed)
        } else {
            io.progress.native_quantum.load(Relaxed)
        }
    })
}

/// Requests a new native output epoch; the `IOProc` discards its own old queue cursor.
/// # Safety
/// IO remains retained; no callback-owned cell is accessed by the requesting owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_output_epoch(io: *mut DeviceIo, epoch: u64) {
    // SAFETY: this control path publishes one atomic request only.
    if let Some(io) = unsafe { io.as_ref() } {
        io.progress.output_epoch.store(epoch, Release);
    }
}

/// Observes clock discontinuities without borrowing callback writer state.
/// # Safety
/// IO remains initialized and retained until this atomic observation ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_clock_discontinuities(io: *mut DeviceIo) -> u64 {
    // SAFETY: shared retained owner and clock's atomic publication only.
    unsafe { io.as_ref() }.map_or(0, |io| unsafe {
        mr_callback_clock_discontinuities(&raw const io.clock)
    })
}

/// Discards only the bridge owner's input consumer cursor, never the graph/native producer.
/// # Safety
/// Sole SRC owner executes this call; IO remains retained and no other consumer overlaps.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_bridge_discard(io: *mut DeviceIo) {
    // SAFETY: immutable owner lookup followed by the bridge-exclusive consumer action.
    if let Some(io) = unsafe { io.as_ref() } {
        unsafe { queues::bridge_discard(io) };
    }
}

/// Reads graph input, discarding old PCM when the graph's recovery epoch changes.
/// Returns actual frames and whether this read admitted the first consumer epoch.
/// `host` is the graph cycle's host time in ticks, shared by all of its transfers.
/// Null IO, wrong direction, zero or oversized requests return the empty default
/// without admitting an epoch. Recovery after a prior read is never initial admission.
/// # Safety
/// Serialized graph consumer, retained IO, and writable frames*channels output
/// floats disjoint from queue storage. Frames above 4096 are rejected without access.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_graph_read(
    io: *mut DeviceIo,
    samples: *mut f32,
    frames: u32,
    epoch: u64,
    host: u64,
) -> queues::GraphRead {
    // SAFETY: graph owns only this consumer and graph epoch cell.
    unsafe { io.as_ref() }.map_or_else(queues::GraphRead::default, |io| unsafe {
        queues::graph_read(io, samples, frames, epoch, (host, None))
    })
}
/// Installs the clock-boundary hook before registration, never on a live callback.
/// # Safety
/// The control owner has no registered `IOProc`. Context stays valid through native
/// unregister; the hook serializes its own mutable state and owns only SRC cursors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_pcm_hook(
    io: *mut DeviceIo,
    context: *mut c_void,
    hook: super::PcmHook,
) {
    // SAFETY: disconnected owner; no callback can borrow this cell yet.
    if let Some(io) = unsafe { io.as_ref() } {
        let callback = unsafe { &mut *io.callback.get() };
        callback.pcm_hook = Some(hook);
        callback.pcm_context = context;
    }
}
