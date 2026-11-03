//! Rust-owned native runtime. Transport layout and callbacks are imported from
//! the common Rust crate; only operating-system calls cross FFI.
use mixrelay_macos_transport::endpoint_transport::{
    EndpointTransport, EndpointTransportConfig as TransportConfig, mr_endpoint_transport_attach,
    mr_endpoint_transport_config, mr_endpoint_transport_incarnation,
};
use mixrelay_macos_transport::hal_io::{
    HalIoContext as IoContext, can_begin_stream, mr_hal_io_begin_stream, mr_hal_io_bind,
    mr_hal_io_clock_state, mr_hal_io_requires_replacement, mr_hal_io_set_render_activity,
};
use mixrelay_macos_transport::transport_inspection::inspect_descriptor;
use std::cell::UnsafeCell;
use std::ffi::c_void;

use crate::abi::{Geometry, HostOps, NativeOps, Timestamp};
use crate::clock::Clock;
use crate::io_outcome::IoOutcome;
use crate::registry::Registry;
mod client_mix;
use client_mix::{ClientMix, PreparedClient, RETIRED_CAPACITY};
use mixrelay_macos_transport::client_pcm::ClientPcmWriter as Queue;
mod pcm_snapshot;
pub(crate) use pcm_snapshot::client_snapshot;

#[repr(C)]
#[derive(Default)]
struct Timebase {
    numerator: u32,
    denominator: u32,
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(output: *mut Timebase) -> i32;
    fn mmap(
        address: *mut c_void,
        size: usize,
        protection: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, size: usize) -> i32;
}

struct Mapping {
    address: *mut c_void,
    size: usize,
    identity: [u8; 16],
}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe { munmap(self.address, self.size) };
    }
}

struct AttachedIo {
    context: UnsafeCell<IoContext>,
    transport: *mut EndpointTransport,
    mapping: Mapping,
    geometry: Geometry,
}

impl Drop for AttachedIo {
    fn drop(&mut self) {
        // Registry control has drained callbacks before retiring an attachment.
        self.context.get_mut().release();
    }
}

impl AttachedIo {
    fn open(geometry: Geometry, fd: i32, size: usize) -> Option<Box<Self>> {
        let address = unsafe { mmap(std::ptr::null_mut(), size, 3, 1, fd, 0) };
        if address as isize == -1 {
            return None;
        }
        let mut mapping = Mapping {
            address,
            size,
            identity: [0; 16],
        };
        let transport = unsafe { mr_endpoint_transport_attach(address, size) };
        let mut config = TransportConfig::default();
        if transport.is_null()
            || !unsafe { mr_endpoint_transport_config(transport, &raw mut config) }
            || config.endpoint_id != geometry.endpoint_id
            || config.render_channels != geometry.render_channels
            || config.capture_channels != geometry.capture_channels
            || !unsafe {
                mr_endpoint_transport_incarnation(transport, mapping.identity.as_mut_ptr())
            }
        {
            return None;
        }
        let context = IoContext::new(geometry.render_channels, geometry.capture_channels)?;
        let result = Box::new(Self {
            context: UnsafeCell::new(context),
            transport,
            mapping,
            geometry,
        });
        Some(result)
    }

    /// Revalidates the header of a mapping prepared earlier; the Driver service owns
    /// its contents and may have reinitialized it since. Reads only, without mapping.
    fn intact(&self) -> bool {
        let mut config = TransportConfig::default();
        // SAFETY: this owner retains the complete mapping; attach only reads its header.
        let transport =
            unsafe { mr_endpoint_transport_attach(self.mapping.address, self.mapping.size) };
        transport == self.transport
            && unsafe { mr_endpoint_transport_config(transport, &raw mut config) }
            && config.endpoint_id == self.geometry.endpoint_id
            && config.render_channels == self.geometry.render_channels
            && config.capture_channels == self.geometry.capture_channels
    }

    fn bind(&self) -> bool {
        unsafe { mr_hal_io_bind(self.context.get(), self.transport.cast()) }
    }
}

struct Runtime {
    clock: Clock,
    io: Box<AttachedIo>,
    clients: ClientMix,
    /// Storage a frozen change detached. IO callbacks never touch it; the registry
    /// releases it with `release` once the freeze has ended, so nothing is unmapped
    /// while callbacks wait.
    retired: UnsafeCell<Retired>,
}

struct Retired {
    io: Option<Box<AttachedIo>>,
    /// Reserved at creation, so detaching queues under the freeze never allocates.
    queues: Vec<Queue>,
    /// Emptied preparation boxes, freed with the rest after the freeze.
    shells: Vec<Shell>,
}

/// Preparations one freeze can consume: an install for every client, and a mapping.
const SHELL_CAPACITY: usize = RETIRED_CAPACITY + 1;

/// Storage mapped on control before the IO freeze that publishes it. The registry owns
/// it until a frozen operation consumes it or `discard` releases it.
enum Change {
    /// A Driver-service endpoint mapping, validated before Perform swaps it in.
    Mapping(Box<AttachedIo>),
    /// One client's queue for registration or its next IO start.
    Client(PreparedClient),
}

/// The heap box a preparation crosses the ABI in. A frozen operation moves its change
/// out and retires the box, so the freeze never frees.
type Shell = Box<Option<Change>>;

/// # Safety
/// `change` is null or was returned by `prepare`/`prepare_client` and not yet consumed.
unsafe fn take(change: *mut c_void) -> Option<Shell> {
    // SAFETY: per the contract above, a non-null pointer owns one boxed change.
    (!change.is_null()).then(|| unsafe { Box::from_raw(change.cast::<Option<Change>>()) })
}

fn boxed(change: Change) -> *mut c_void {
    Box::into_raw(Box::new(Some(change))).cast()
}

/// Takes the change out of `change` inside a freeze, keeping its emptied box in
/// `runtime` for `release`.
/// # Safety
/// As for `take`.
unsafe fn unbox(runtime: &mut Runtime, change: *mut c_void) -> Option<Change> {
    // SAFETY: forwarded from the caller.
    let mut shell = unsafe { take(change) }?;
    let change = shell.take();
    let shells = &mut runtime.retired.get_mut().shells;
    if shells.len() < shells.capacity() {
        shells.push(shell);
    }
    change
}

impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe { mr_hal_io_clock_state(self.io.context.get(), self.clock.rate, false) };
    }
}

unsafe extern "C" fn inspect(fd: i32, size: usize, output: *mut Geometry) -> bool {
    let Ok(config) = inspect_descriptor(fd, size) else {
        return false;
    };
    unsafe {
        output.write(Geometry {
            endpoint_id: config.endpoint_id,
            render_channels: config.render_channels,
            capture_channels: config.capture_channels,
        });
    };
    true
}

unsafe extern "C" fn create(
    geometry: *const Geometry,
    rate: u32,
    fd: i32,
    size: usize,
) -> *mut c_void {
    let mut timebase = Timebase::default();
    if unsafe { mach_timebase_info(&raw mut timebase) } != 0 {
        return std::ptr::null_mut();
    }
    let Some(clock) = Clock::new(rate, timebase.numerator, timebase.denominator) else {
        return std::ptr::null_mut();
    };
    let Some(io) = AttachedIo::open(unsafe { *geometry }, fd, size) else {
        return std::ptr::null_mut();
    };
    if !io.bind() {
        return std::ptr::null_mut();
    }
    unsafe { mr_hal_io_clock_state(io.context.get(), rate, false) };
    let Some(clients) = ClientMix::new(unsafe { *geometry }) else {
        return std::ptr::null_mut();
    };
    let mut queues = Vec::new();
    let mut shells = Vec::new();
    if queues.try_reserve_exact(RETIRED_CAPACITY).is_err()
        || shells.try_reserve_exact(SHELL_CAPACITY).is_err()
    {
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(Runtime {
        clock,
        io,
        clients,
        retired: UnsafeCell::new(Retired {
            io: None,
            queues,
            shells,
        }),
    }))
    .cast()
}

unsafe extern "C" fn destroy(runtime: *mut c_void) {
    drop(unsafe { Box::from_raw(runtime.cast::<Runtime>()) });
}

unsafe extern "C" fn prepare(geometry: *const Geometry, fd: i32, size: usize) -> *mut c_void {
    // SAFETY: the registry passes its validated attachment geometry for this call.
    AttachedIo::open(unsafe { *geometry }, fd, size)
        .map_or(std::ptr::null_mut(), |io| boxed(Change::Mapping(io)))
}

unsafe extern "C" fn prepare_client(
    runtime: *mut c_void,
    client: u32,
    change: *mut *mut c_void,
) -> bool {
    // SAFETY: registry control excludes every runtime mutation. IO callbacks share this
    // runtime concurrently; preparation only reads the registered client set.
    let runtime = unsafe { &*runtime.cast::<Runtime>() };
    let clients = &runtime.clients;
    // SAFETY: the registry passes one writable output for this call.
    unsafe { change.write(std::ptr::null_mut()) };
    if !clients.needs_queue(client) {
        return true;
    }
    let Some(prepared) = clients.prepare(client) else {
        return false;
    };
    // SAFETY: as above.
    unsafe { change.write(boxed(Change::Client(prepared))) };
    true
}

unsafe extern "C" fn discard(change: *mut c_void) {
    // SAFETY: the registry releases each unconsumed preparation once, outside IO.
    drop(unsafe { take(change) });
}

unsafe extern "C" fn release(runtime: *mut c_void) {
    // SAFETY: the registry calls this on control after the freeze ended. IO callbacks
    // share the runtime but never read `retired`; control serializes its writers.
    let retired = unsafe { &mut *(*runtime.cast::<Runtime>()).retired.get() };
    drop(retired.io.take());
    // Clearing keeps each reservation for the next freeze.
    retired.queues.clear();
    retired.shells.clear();
}

/// Retains `change` for release after the freeze when a frozen operation refused it.
fn retire(runtime: &mut Runtime, change: Option<Change>) {
    let retired = runtime.retired.get_mut();
    match change {
        Some(Change::Mapping(io)) => retired.io = Some(io),
        Some(Change::Client(prepared)) => {
            if let Some(queue) = prepared.into_queue()
                && retired.queues.len() < retired.queues.capacity()
            {
                retired.queues.push(queue);
            }
        }
        None => {}
    }
}

unsafe extern "C" fn replace(runtime: *mut c_void, change: *mut c_void, rate: u32) -> bool {
    let runtime = unsafe { &mut *runtime.cast::<Runtime>() };
    // SAFETY: the registry passes ownership of its prepared mapping exactly once.
    let Some(Change::Mapping(candidate)) = (unsafe { unbox(runtime, change) }) else {
        return false;
    };
    let result = swap_mapping(runtime, candidate, rate);
    let committed = result.is_ok();
    // The previous mapping, or the refused candidate, is released after the freeze.
    let (Ok(previous) | Err(previous)) = result;
    runtime.retired.get_mut().io = Some(previous);
    committed
}

/// Swaps a validated endpoint mapping in and adopts its geometry. No client queue is
/// mapped here: queues of another shape detach, and each client maps a fresh one at its
/// next IO start. Every fallible step precedes the first change to published state.
fn swap_mapping(
    runtime: &mut Runtime,
    mut candidate: Box<AttachedIo>,
    rate: u32,
) -> Result<Box<AttachedIo>, Box<AttachedIo>> {
    let mut clock = runtime.clock.clone();
    if !candidate.intact() || !clock.set_rate(rate, unsafe { mach_absolute_time() }) {
        return Err(candidate);
    }
    let previous = unsafe { &*runtime.io.context.get() };
    let context = unsafe { &mut *candidate.context.get() };
    context.inherit_lifecycle(previous);
    let running = runtime.clock.running;
    // The candidate is not yet published, so binding it changes no live state.
    if !candidate.bind() || (running && !unsafe { can_begin_stream(candidate.context.get()) }) {
        return Err(candidate);
    }
    let retired = &mut runtime.retired.get_mut().queues;
    if !runtime
        .clients
        .publish_clock_after(&clock, true, retired, || {
            // SAFETY: control owns the drained candidate, checked above.
            !running || unsafe { mr_hal_io_begin_stream(candidate.context.get()) }
        })
    {
        return Err(candidate);
    }
    runtime
        .clients
        .reconfigure(candidate.geometry, &mut runtime.retired.get_mut().queues);
    runtime.clock = clock;
    unsafe { mr_hal_io_clock_state(candidate.context.get(), rate, runtime.clock.running) };
    if candidate.mapping.identity != runtime.io.mapping.identity {
        unsafe { mr_hal_io_clock_state(runtime.io.context.get(), rate, false) };
    }
    std::mem::swap(&mut runtime.io, &mut candidate);
    Ok(candidate)
}

unsafe extern "C" fn start(runtime: *mut c_void, now: u64) -> bool {
    let runtime = unsafe { &mut *runtime.cast::<Runtime>() };
    let mut clock = runtime.clock.clone();
    let retired = &mut runtime.retired.get_mut().queues;
    // SAFETY: registry control owns this runtime and drained every IO pin. The shared
    // client clock stays hidden until the endpoint rings rearm, and no client queue
    // changes before that.
    if !clock.start(now)
        || !unsafe { can_begin_stream(runtime.io.context.get()) }
        || !runtime
            .clients
            .publish_clock_after(&clock, true, retired, || unsafe {
                mr_hal_io_begin_stream(runtime.io.context.get())
            })
    {
        return false;
    }
    runtime.clock = clock;
    unsafe { mr_hal_io_clock_state(runtime.io.context.get(), runtime.clock.rate, true) };
    true
}

unsafe extern "C" fn stop(runtime: *mut c_void) {
    let runtime = unsafe { &mut *runtime.cast::<Runtime>() };
    runtime.clock.running = false;
    let retired = &mut runtime.retired.get_mut().queues;
    let _ = runtime
        .clients
        .publish_clock(&runtime.clock, false, retired);
    unsafe {
        mr_hal_io_clock_state(runtime.io.context.get(), runtime.clock.rate, false);
        mr_hal_io_set_render_activity(runtime.io.context.get(), 1);
    }
}

unsafe extern "C" fn remaining(runtime: *mut c_void) {
    let runtime = unsafe { &*runtime.cast::<Runtime>() };
    unsafe { mr_hal_io_set_render_activity(runtime.io.context.get(), 0) };
}

unsafe extern "C" fn set_rate(runtime: *mut c_void, rate: u32) -> bool {
    let runtime = unsafe { &mut *runtime.cast::<Runtime>() };
    let mut clock = runtime.clock.clone();
    let running = runtime.clock.running;
    let retired = &mut runtime.retired.get_mut().queues;
    // Queues keep their mappings and grants. Rearming advances every ring epoch and the
    // shared clock together, so readers discard each frame committed at the old rate;
    // a queue without a spare epoch detaches until its client next starts IO. The
    // endpoint rings are checked, and rearmed, before any client queue changes.
    // SAFETY: registry control owns this runtime and drained every IO pin.
    if !clock.set_rate(rate, unsafe { mach_absolute_time() })
        || (running && !unsafe { can_begin_stream(runtime.io.context.get()) })
        || !runtime
            .clients
            .publish_clock_after(&clock, true, retired, || {
                // SAFETY: as above.
                !running || unsafe { mr_hal_io_begin_stream(runtime.io.context.get()) }
            })
    {
        return false;
    }
    runtime.clock = clock;
    unsafe { mr_hal_io_clock_state(runtime.io.context.get(), rate, runtime.clock.running) };
    true
}

unsafe extern "C" fn timestamp(runtime: *mut c_void, now: u64, output: *mut Timestamp) -> bool {
    unsafe { &*runtime.cast::<Runtime>() }
        .clock
        .timestamp(now, unsafe { &mut *output })
}

unsafe extern "C" fn io(
    runtime: *mut c_void,
    render: bool,
    client: u32,
    frame: u64,
    samples: *mut f32,
    count: u32,
) -> IoOutcome {
    // SAFETY: the registry's IO pin retains this initialized native runtime.
    let runtime = unsafe { &*runtime.cast::<Runtime>() };
    let outcome = if render {
        // SAFETY: MixOutput supplies canonical Float32 PCM for this registered client.
        runtime.clients.render(
            client,
            frame,
            unsafe {
                std::slice::from_raw_parts(
                    samples,
                    count as usize
                        * runtime
                            .io
                            .context
                            .get()
                            .as_ref()
                            .map_or(0, |context| context.render_channels)
                            as usize,
                )
            },
            count,
        )
    } else {
        // SAFETY: HAL provides count * capture channels writable Float32 samples.
        runtime.clients.capture(
            frame,
            unsafe {
                std::slice::from_raw_parts_mut(
                    samples,
                    count as usize
                        * runtime
                            .io
                            .context
                            .get()
                            .as_ref()
                            .map_or(0, |context| context.capture_channels)
                            as usize,
                )
            },
            count,
        )
    };
    if outcome.accepted && count != 0 {
        // SAFETY: this pinned runtime retains the validated transport. Progress is
        // published after the client queue, independently of any postmix callback.
        unsafe {
            mixrelay_macos_transport::endpoint_transport::mr_endpoint_transport_clock_demand(
                runtime.io.transport,
                render,
                frame + u64::from(count),
                count,
            );
            if render {
                mr_hal_io_set_render_activity(runtime.io.context.get(), 3);
            }
        }
    }
    outcome
}

unsafe extern "C" fn client(
    runtime: *mut c_void,
    client: u32,
    action: u32,
    change: *mut c_void,
) -> bool {
    // SAFETY: registry control holds the device freeze, draining every IO pin.
    let runtime = unsafe { &mut *runtime.cast::<Runtime>() };
    // SAFETY: registration and IO start pass ownership of their prepared queue.
    let change = unsafe { unbox(runtime, change) };
    let change = match crate::io_outcome::ClientChange::from_raw(action) {
        Some(crate::io_outcome::ClientChange::Register) => {
            // Without a preparation the client registers without a queue.
            let mut prepared = match change {
                Some(Change::Client(prepared)) => prepared,
                None => PreparedClient::none(client),
                Some(change) => {
                    retire(runtime, Some(change));
                    return false;
                }
            };
            let registered = prepared.id() == client && runtime.clients.register(&mut prepared);
            let retired = &mut runtime.retired.get_mut().queues;
            let published = registered
                && runtime
                    .clients
                    .publish_clock(&runtime.clock, false, retired);
            if registered && !published {
                prepared = PreparedClient::detached(client, runtime.clients.remove(client));
            }
            retire(runtime, Some(Change::Client(prepared)));
            return published;
        }
        Some(crate::io_outcome::ClientChange::Install) => {
            let mut prepared = match change {
                Some(Change::Client(prepared)) => prepared,
                // The client already has a queue it can use.
                None => return true,
                Some(change) => {
                    retire(runtime, Some(change));
                    return false;
                }
            };
            let retired = &mut runtime.retired.get_mut().queues;
            let installed = prepared.id() == client
                && runtime.clients.install(&mut prepared, retired)
                && runtime
                    .clients
                    .publish_clock(&runtime.clock, false, retired);
            retire(runtime, Some(Change::Client(prepared)));
            return installed;
        }
        Some(crate::io_outcome::ClientChange::Remove) => Change::Client(PreparedClient::detached(
            client,
            runtime.clients.remove(client),
        )),
        None => return false,
    };
    retire(runtime, Some(change));
    true
}

unsafe extern "C" fn requires_replacement(runtime: *mut c_void) -> bool {
    let runtime = unsafe { &*runtime.cast::<Runtime>() };
    unsafe { mr_hal_io_requires_replacement(runtime.io.context.get()) }
}

unsafe extern "C" fn same_transport(runtime: *mut c_void, fd: i32, size: usize) -> bool {
    let runtime = unsafe { &*runtime.cast::<Runtime>() };
    let address = unsafe { mmap(std::ptr::null_mut(), size, 3, 1, fd, 0) };
    if address as isize == -1 {
        return false;
    }
    let mut candidate = Mapping {
        address,
        size,
        identity: [0; 16],
    };
    let transport = unsafe { mr_endpoint_transport_attach(address, size) };
    let valid =
        unsafe { mr_endpoint_transport_incarnation(transport, candidate.identity.as_mut_ptr()) };
    valid && candidate.identity == runtime.io.mapping.identity
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_hal_core_create_native(host: *const HostOps) -> *mut Registry {
    let Some(host) = (unsafe { host.as_ref() }) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(native_registry(
        *host,
        crate::transport::ProcessInspection::SYSTEM,
    )))
}

/// The registry over the native device operations, inspecting processes through `inspection`.
pub(crate) fn native_registry(
    host: HostOps,
    inspection: crate::transport::ProcessInspection,
) -> Registry {
    let native = NativeOps {
        inspect,
        create,
        destroy,
        prepare,
        discard,
        release,
        replace,
        start,
        stop,
        remaining_client: remaining,
        set_rate,
        timestamp,
        prepare_client,
        client,
        io,
        requires_replacement,
        same_transport,
    };
    Registry::with_inspection(native, host, inspection, mixrelay_time::StdClock::new())
}
