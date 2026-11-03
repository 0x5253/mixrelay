use std::ffi::c_void;
use std::io::{Read, Seek, SeekFrom, Write};
use std::mem::ManuallyDrop;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::abi::{
    CAPACITY, CONFIG_ACTION, Geometry, HostOps, INVALID, NATIVE_ERROR, NOT_FOUND, NativeOps, OK,
    Timestamp,
};
use crate::device::lock;
use crate::registry::{IoOperation, Registry};

#[path = "clock_property_tests.rs"]
mod clock_property_tests;
#[path = "inventory_tests.rs"]
mod inventory_tests;
#[path = "io_admission/tests.rs"]
mod io_admission_tests;
#[path = "native_tests.rs"]
mod native_tests;
#[path = "property_catalog_tests.rs"]
mod property_catalog_tests;

static SERIAL: Mutex<()> = Mutex::new(());
static NEXT: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static REPLACEMENTS: AtomicUsize = AtomicUsize::new(0);
static PREPARATIONS: AtomicUsize = AtomicUsize::new(0);
static PREPARED_DURING_IO: AtomicUsize = AtomicUsize::new(0);
static RELEASES: AtomicUsize = AtomicUsize::new(0);
static FAIL_REPLACE: AtomicBool = AtomicBool::new(false);
static FAIL_PREPARE_CLIENT: AtomicBool = AtomicBool::new(false);
static FAIL_CREATE_ENDPOINT: AtomicU8 = AtomicU8::new(0);
static PAUSE_IO: AtomicBool = AtomicBool::new(false);
static IO_ENTERED: AtomicBool = AtomicBool::new(false);
static RELEASE_IO: AtomicBool = AtomicBool::new(false);

struct Runtime {
    geometry: Geometry,
    rate: u32,
    running: bool,
    epoch: u64,
    value: f32,
}

unsafe extern "C" fn inspect(fd: i32, size: usize, output: *mut Geometry) -> bool {
    if size != 24 {
        return false;
    }
    let mut file = ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
    let mut bytes = [0; 24];
    if file
        .seek(SeekFrom::Start(0))
        .and_then(|_| file.read_exact(&mut bytes))
        .is_err()
    {
        return false;
    }
    unsafe {
        output.write(Geometry {
            endpoint_id: bytes[..16].try_into().unwrap(),
            render_channels: u32::from_ne_bytes(bytes[16..20].try_into().unwrap()),
            capture_channels: u32::from_ne_bytes(bytes[20..24].try_into().unwrap()),
        });
    };
    true
}

unsafe extern "C" fn create(geometry: *const Geometry, rate: u32, _: i32, _: usize) -> *mut c_void {
    if unsafe { (*geometry).endpoint_id[0] } == FAIL_CREATE_ENDPOINT.load(Ordering::SeqCst) {
        return std::ptr::null_mut();
    }
    LIVE.fetch_add(1, Ordering::SeqCst);
    Box::into_raw(Box::new(Runtime {
        geometry: unsafe { *geometry },
        rate,
        running: false,
        epoch: 0,
        value: 0.0,
    }))
    .cast()
}
unsafe extern "C" fn destroy(runtime: *mut c_void) {
    drop(unsafe { Box::from_raw(runtime.cast::<Runtime>()) });
    LIVE.fetch_sub(1, Ordering::SeqCst);
}
/// A fake preparation records when control made it relative to in-flight IO.
enum Change {
    Mapping(Geometry),
    Client(u32),
}
fn change(change: Change) -> *mut c_void {
    if IO_ENTERED.load(Ordering::Acquire) && !RELEASE_IO.load(Ordering::Acquire) {
        PREPARED_DURING_IO.fetch_add(1, Ordering::SeqCst);
    }
    PREPARATIONS.fetch_add(1, Ordering::SeqCst);
    Box::into_raw(Box::new(change)).cast()
}
fn take(change: *mut c_void) -> Option<Change> {
    (!change.is_null()).then(|| *unsafe { Box::from_raw(change.cast::<Change>()) })
}
unsafe extern "C" fn prepare(geometry: *const Geometry, _: i32, _: usize) -> *mut c_void {
    change(Change::Mapping(unsafe { *geometry }))
}
unsafe extern "C" fn prepare_client(
    _: *mut c_void,
    client: u32,
    prepared: *mut *mut c_void,
) -> bool {
    if FAIL_PREPARE_CLIENT.load(Ordering::SeqCst) {
        return false;
    }
    unsafe { prepared.write(change(Change::Client(client))) };
    true
}
unsafe extern "C" fn release(_: *mut c_void) {
    RELEASES.fetch_add(1, Ordering::SeqCst);
}
unsafe extern "C" fn discard(change: *mut c_void) {
    take(change);
}
unsafe extern "C" fn replace(runtime: *mut c_void, change: *mut c_void, rate: u32) -> bool {
    let Some(Change::Mapping(geometry)) = take(change) else {
        return false;
    };
    if FAIL_REPLACE.load(Ordering::SeqCst) {
        return false;
    }
    let runtime = unsafe { &mut *runtime.cast::<Runtime>() };
    runtime.geometry = geometry;
    runtime.rate = rate;
    runtime.epoch += 1;
    REPLACEMENTS.fetch_add(1, Ordering::SeqCst);
    true
}
unsafe extern "C" fn start(runtime: *mut c_void, _: u64) -> bool {
    let runtime = unsafe { &mut *runtime.cast::<Runtime>() };
    runtime.running = true;
    runtime.epoch += 1;
    true
}
unsafe extern "C" fn stop(runtime: *mut c_void) {
    unsafe { &mut *runtime.cast::<Runtime>() }.running = false;
}
unsafe extern "C" fn remaining(_: *mut c_void) {}
unsafe extern "C" fn requires_replacement(_: *mut c_void) -> bool {
    false
}
unsafe extern "C" fn same_transport(_: *mut c_void, _: i32, _: usize) -> bool {
    false
}
unsafe extern "C" fn set_rate(runtime: *mut c_void, rate: u32) -> bool {
    unsafe { &mut *runtime.cast::<Runtime>() }.rate = rate;
    true
}
unsafe extern "C" fn timestamp(runtime: *mut c_void, now: u64, output: *mut Timestamp) -> bool {
    let runtime = unsafe { &*runtime.cast::<Runtime>() };
    unsafe {
        output.write(Timestamp {
            sample_time: u64::from(runtime.rate),
            host_time: now,
            seed: runtime.epoch,
        });
    };
    runtime.running
}
unsafe extern "C" fn native_client(
    _: *mut c_void,
    client: u32,
    action: u32,
    change: *mut c_void,
) -> bool {
    use crate::io_outcome::ClientChange;
    match (ClientChange::from_raw(action), take(change)) {
        (Some(ClientChange::Register | ClientChange::Remove), None) => true,
        (Some(ClientChange::Register | ClientChange::Install), Some(Change::Client(prepared))) => {
            prepared == client
        }
        _ => false,
    }
}
unsafe extern "C" fn io(
    runtime: *mut c_void,
    render: bool,
    _: u32,
    _: u64,
    samples: *mut f32,
    frames: u32,
) -> crate::io_outcome::IoOutcome {
    if PAUSE_IO.load(Ordering::Acquire) {
        IO_ENTERED.store(true, Ordering::Release);
        while !RELEASE_IO.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
    }
    let runtime = unsafe { &mut *runtime.cast::<Runtime>() };
    if render {
        runtime.value = unsafe { *samples };
    } else {
        for index in 0..frames as usize * runtime.geometry.capture_channels as usize {
            unsafe { *samples.add(index) = runtime.value };
        }
    }
    crate::io_outcome::IoOutcome::completed(frames)
}

#[test]
fn standalone_ffi_audio_entries_restore_the_callers_thread_scope() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    assert_eq!(fixture.registry.client(id, 7, true), OK);
    assert_eq!(fixture.registry.start(id, 7, 0), OK);
    let registry = (&raw const *fixture.registry).cast_mut();
    let mut samples = [0.25_f32; 2];
    let mut timestamp = crate::abi::Timestamp::default();
    assert!(!crate::transport::on_audio_thread());
    assert!(unsafe {
        crate::abi::mr_hal_core_io(
            registry,
            id,
            id + 2,
            true,
            42,
            0,
            samples.as_mut_ptr().cast(),
            1,
        )
    });
    assert!(!crate::transport::on_audio_thread());
    assert!(unsafe { crate::abi::mr_hal_core_timestamp(registry, id, 0, &raw mut timestamp) });
    assert!(!crate::transport::on_audio_thread());
    assert!(!unsafe {
        crate::abi::mr_hal_core_io(registry, id, id + 2, true, 42, 0, std::ptr::null_mut(), 1)
    });
    assert!(!crate::transport::on_audio_thread());
    assert!(!unsafe {
        crate::abi::mr_hal_core_timestamp(std::ptr::null_mut(), id, 0, &raw mut timestamp)
    });
    assert!(!crate::transport::on_audio_thread());
}

#[test]
fn stream_usage_reconciliation_notifies_only_actual_transitions() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    lock(&fixture.host.changes).clear();
    fixture
        .host
        .reconcile_stream_usage
        .store(true, Ordering::Release);
    for stream in [id + 1, id + 2] {
        assert_eq!(fixture.registry.set_stream_active(stream, true), OK);
    }
    assert!(lock(&fixture.host.changes).is_empty());
    for stream in [id + 1, id + 2] {
        assert_eq!(fixture.registry.set_stream_active(stream, false), OK);
        assert_eq!(fixture.registry.set_stream_active(stream, false), OK);
        assert_eq!(fixture.registry.set_stream_active(stream, true), OK);
    }
    assert_eq!(
        &*lock(&fixture.host.changes),
        &[
            (id + 1, crate::abi::STREAM_ACTIVE),
            (id + 1, crate::abi::STREAM_ACTIVE),
            (id + 2, crate::abi::STREAM_ACTIVE),
            (id + 2, crate::abi::STREAM_ACTIVE),
        ]
    );
}

struct Host {
    requests: Mutex<Vec<(u32, u64)>>,
    changes: Mutex<Vec<(u32, u32)>>,
    fail: AtomicBool,
    registry: AtomicPtr<Registry>,
    replace_during_request: AtomicBool,
    reconcile_stream_usage: AtomicBool,
}
unsafe extern "C" fn changed(context: *mut c_void, object: u32, change: u32) {
    let host = unsafe { &*context.cast::<Host>() };
    lock(&host.changes).push((object, change));
    if let Some(registry) = unsafe { host.registry.load(Ordering::Acquire).as_ref() } {
        let _ = registry.devices();
        if change == crate::abi::STREAM_ACTIVE
            && host.reconcile_stream_usage.load(Ordering::Acquire)
            && lock(&host.changes).len() < 8
        {
            let snapshot = registry.snapshot(object).unwrap();
            let active = if object == snapshot.input_stream_id {
                snapshot.input_active
            } else {
                snapshot.output_active
            };
            assert_eq!(registry.set_stream_active(object, active), OK);
        }
        if change == crate::abi::INPUT_LATENCY {
            assert!(registry.inventory.try_lock().is_ok());
            assert!(registry.client_identity_gate.try_lock().is_ok());
            let device = registry.pin(object).unwrap();
            assert!(device.control.try_lock().is_ok());
        }
        if change == crate::abi::TRANSPORT {
            assert!(registry.inventory.try_lock().is_ok());
            assert!(registry.transport.state.try_lock().is_ok());
            // Reentrant queries prove notification delivery owns no resolver lock.
            assert_eq!(registry.transport.get(object, 0), crate::transport::VIRTUAL);
        }
    }
}
unsafe extern "C" fn request(
    context: *mut c_void,
    object: u32,
    action: u64,
    generation: u64,
) -> bool {
    assert_eq!(action, CONFIG_ACTION);
    let host = unsafe { &*context.cast::<Host>() };
    lock(&host.requests).push((object, generation));
    if host.replace_during_request.swap(false, Ordering::SeqCst) {
        let registry = unsafe { &*host.registry.load(Ordering::Acquire) };
        assert_eq!(registry.set_rate(object, 96_000), OK);
    }
    !host.fail.load(Ordering::SeqCst)
}

struct Fixture {
    registry: Box<Registry>,
    host: Box<Host>,
    directory: PathBuf,
    _serial: MutexGuard<'static, ()>,
}
impl Fixture {
    fn new() -> Self {
        let serial = lock(&SERIAL);
        assert_eq!(LIVE.load(Ordering::SeqCst), 0);
        FAIL_REPLACE.store(false, Ordering::SeqCst);
        FAIL_PREPARE_CLIENT.store(false, Ordering::SeqCst);
        FAIL_CREATE_ENDPOINT.store(0, Ordering::SeqCst);
        REPLACEMENTS.store(0, Ordering::SeqCst);
        PREPARATIONS.store(0, Ordering::SeqCst);
        PREPARED_DURING_IO.store(0, Ordering::SeqCst);
        RELEASES.store(0, Ordering::SeqCst);
        PAUSE_IO.store(false, Ordering::SeqCst);
        IO_ENTERED.store(false, Ordering::SeqCst);
        RELEASE_IO.store(false, Ordering::SeqCst);
        let host = Box::new(Host {
            requests: Mutex::new(Vec::new()),
            changes: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
            registry: AtomicPtr::default(),
            replace_during_request: AtomicBool::new(false),
            reconcile_stream_usage: AtomicBool::new(false),
        });
        let ops = NativeOps {
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
            client: native_client,
            io,
            requires_replacement,
            same_transport,
        };
        // The installed driver host cannot inspect other processes; every fixture starts
        // from that constraint.
        let registry = Box::new(Registry::with_inspection(
            ops,
            HostOps {
                context: (&raw const *host).cast_mut().cast(),
                changed,
                request_configuration: request,
            },
            crate::transport::ProcessInspection::SANDBOXED,
            mixrelay_time::StdClock::new(),
        ));
        host.registry
            .store((&raw const *registry).cast_mut(), Ordering::Release);
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../target/test-artifacts/hal-core")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
        std::fs::create_dir_all(&directory).unwrap();
        Self {
            registry,
            host,
            directory,
            _serial: serial,
        }
    }

    fn fd(&self, endpoint: u8, render: u32, capture: u32) -> OwnedFd {
        let path = self
            .directory
            .join(NEXT.fetch_add(1, Ordering::SeqCst).to_string());
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        file.write_all(&[endpoint; 16]).unwrap();
        file.write_all(&render.to_ne_bytes()).unwrap();
        file.write_all(&capture.to_ne_bytes()).unwrap();
        file.into()
    }

    fn add(&self, endpoint: u8, rate: u32) -> u32 {
        self.registry
            .upsert(
                [endpoint; 16],
                &format!("Device {endpoint}"),
                rate,
                (self.fd(endpoint, 2, 2), 24),
            )
            .unwrap()
    }

    fn pending(&self) -> (u32, u64) {
        *lock(&self.host.requests).last().unwrap()
    }
    fn perform(&self) -> i32 {
        let (id, generation) = self.pending();
        self.registry.perform(id, CONFIG_ACTION, generation)
    }

    /// Reads a property as coreaudiod itself (requester PID 0).
    fn property<const N: usize>(
        &self,
        object: u32,
        address: &crate::properties::Address,
    ) -> [u8; N] {
        self.property_for(object, address, 0)
    }
    fn property_for<const N: usize>(
        &self,
        object: u32,
        address: &crate::properties::Address,
        requester: i32,
    ) -> [u8; N] {
        let mut bytes = [0; N];
        let mut written = 0;
        assert_eq!(
            unsafe {
                crate::properties::mr_hal_core_property_get(
                    (&raw const *self.registry).cast_mut(),
                    object,
                    address,
                    0,
                    std::ptr::null(),
                    u32::try_from(N).unwrap(),
                    &raw mut written,
                    bytes.as_mut_ptr().cast(),
                    requester,
                )
            },
            0
        );
        assert_eq!(written as usize, N);
        bytes
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
fn independent_devices_enumerate_and_route_without_cross_talk() {
    let fixture = Fixture::new();
    let a = fixture.add(1, 48_000);
    let b = fixture.add(2, 96_000);
    assert_eq!(fixture.registry.devices(), [a, b]);
    for id in [a, b] {
        assert_eq!(fixture.registry.client(id, 3, true), OK);
        assert_eq!(fixture.registry.start(id, 3, 100), OK);
    }
    let mut first = [0.25_f32; 64];
    let mut second = [0.75_f32; 64];
    unsafe {
        assert!(fixture.registry.io(IoOperation {
            client: 42,
            device: a,
            stream: a + 2,
            render: true,
            frame: 0,
            samples: first.as_mut_ptr().cast(),
            frames: 32
        }));
        assert!(fixture.registry.io(IoOperation {
            client: 42,
            device: b,
            stream: b + 2,
            render: true,
            frame: 0,
            samples: second.as_mut_ptr().cast(),
            frames: 32
        }));
        assert!(fixture.registry.io(IoOperation {
            client: 42,
            device: a,
            stream: a + 1,
            render: false,
            frame: 0,
            samples: second.as_mut_ptr().cast(),
            frames: 32
        }));
        assert!(fixture.registry.io(IoOperation {
            client: 42,
            device: b,
            stream: b + 1,
            render: false,
            frame: 0,
            samples: first.as_mut_ptr().cast(),
            frames: 32
        }));
    }
    assert_eq!(first.map(f32::to_bits), [0.75_f32.to_bits(); 64]);
    assert_eq!(second.map(f32::to_bits), [0.25_f32.to_bits(); 64]);
    assert_eq!(fixture.registry.snapshot(a).unwrap().sample_rate, 48_000);
    assert_eq!(fixture.registry.snapshot(b).unwrap().sample_rate, 96_000);
}

#[test]
fn stale_configuration_callbacks_cannot_drop_new_transport() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    fixture
        .registry
        .upsert([1; 16], "Old", 48_000, (fixture.fd(1, 7, 3), 24))
        .unwrap();
    let old = fixture.pending().1;
    fixture
        .registry
        .upsert([1; 16], "Newest", 48_000, (fixture.fd(1, 13, 33), 24))
        .unwrap();
    assert_eq!(fixture.registry.abort(id, CONFIG_ACTION, old), OK);
    assert_eq!(fixture.registry.perform(id, CONFIG_ACTION, old), OK);
    assert_eq!(fixture.perform(), OK);
    assert_eq!(fixture.perform(), OK);
    let snapshot = fixture.registry.snapshot(id).unwrap();
    assert_eq!(
        (snapshot.render_channels, snapshot.capture_channels),
        (13, 33)
    );
    assert_eq!(&snapshot.name[..6], b"Newest");
    assert_eq!(REPLACEMENTS.load(Ordering::SeqCst), 1);
}

#[test]
fn pending_descriptor_ownership_closes_superseded_and_aborted_fds() {
    use std::os::fd::AsRawFd;
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let first = fixture.fd(1, 7, 3);
    let old_fd = first.as_raw_fd();
    let old_file = file_identity(old_fd).unwrap();
    fixture
        .registry
        .upsert([1; 16], "Old", 48_000, (first, 24))
        .unwrap();
    assert_eq!(file_identity(old_fd), Some(old_file));
    let second = fixture.fd(1, 8, 2);
    let mut new_fd = second.as_raw_fd();
    let mut new_file = file_identity(new_fd).unwrap();
    fixture
        .registry
        .upsert([1; 16], "New", 48_000, (second, 24))
        .unwrap();
    assert_ne!(file_identity(old_fd), Some(old_file));
    assert_eq!(file_identity(new_fd), Some(new_file));
    for _ in 0..1000 {
        let next = fixture.fd(1, 8, 2);
        let next_fd = next.as_raw_fd();
        let next_file = file_identity(next_fd).unwrap();
        fixture
            .registry
            .upsert([1; 16], "Newest", 48_000, (next, 24))
            .unwrap();
        assert_ne!(file_identity(new_fd), Some(new_file));
        (new_fd, new_file) = (next_fd, next_file);
    }
    assert_eq!(
        fixture
            .registry
            .abort(id, CONFIG_ACTION, fixture.pending().1),
        OK
    );
    assert_ne!(file_identity(new_fd), Some(new_file));
}

#[test]
fn failed_host_requests_retry_and_new_generation_resets_backoff() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    fixture.host.fail.store(true, Ordering::SeqCst);
    fixture
        .registry
        .upsert([1; 16], "Pending", 48_000, (fixture.fd(1, 7, 3), 24))
        .unwrap();
    for _ in 0..1000 {
        fixture.registry.tick();
    }
    assert!(lock(&fixture.host.requests).len() > 10);
    fixture.host.fail.store(false, Ordering::SeqCst);
    fixture
        .registry
        .upsert([1; 16], "Recovered", 48_000, (fixture.fd(1, 8, 2), 24))
        .unwrap();
    assert_eq!(fixture.perform(), OK);
    assert_eq!(fixture.registry.snapshot(id).unwrap().render_channels, 8);
}

#[test]
fn acknowledged_configuration_without_perform_retries_until_cancelled() {
    use crate::registry::CONFIG_ACK_TIMEOUT_TICKS;

    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    assert_eq!(fixture.registry.set_rate(id, 96_000), OK);
    let token = fixture.pending();
    for _ in 0..CONFIG_ACK_TIMEOUT_TICKS - 1 {
        fixture.registry.tick();
    }
    assert_eq!(lock(&fixture.host.requests).len(), 1);
    fixture.registry.tick();
    assert_eq!(&*lock(&fixture.host.requests), &[token, token]);
    for _ in 0..1000 {
        fixture.registry.tick();
    }
    assert_eq!(lock(&fixture.host.requests).len(), 22);
    assert_eq!(fixture.registry.abort(id, CONFIG_ACTION, token.1), OK);
    for _ in 0..CONFIG_ACK_TIMEOUT_TICKS * 2 {
        fixture.registry.tick();
    }
    assert_eq!(lock(&fixture.host.requests).len(), 22);
    assert_eq!(fixture.registry.snapshot(id).unwrap().sample_rate, 48_000);
}

#[test]
fn reentrant_replacement_failure_cannot_backoff_the_new_generation_twice() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    fixture.host.fail.store(true, Ordering::SeqCst);
    fixture
        .host
        .replace_during_request
        .store(true, Ordering::SeqCst);
    assert_eq!(fixture.registry.set_rate(id, 44_100), OK);
    let requests = lock(&fixture.host.requests).clone();
    assert_eq!(requests.len(), 2);
    assert_ne!(requests[0].1, requests[1].1);
    let device = fixture.registry.pin(id).unwrap();
    let control = lock(&device.control);
    let pending = control.pending.as_ref().unwrap();
    assert_eq!((pending.failures, pending.retry_after), (1, 2));
    assert_eq!(pending.rate, 96_000);
    drop(control);
    drop(device);
    fixture.host.fail.store(false, Ordering::SeqCst);
    fixture.registry.tick();
    fixture.registry.tick();
    assert_eq!(fixture.perform(), OK);
    assert_eq!(fixture.registry.snapshot(id).unwrap().sample_rate, 96_000);
}

#[test]
fn remove_keeps_client_lifetime_but_never_reuses_stale_object_id() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    assert_eq!(fixture.registry.client(id, 4, true), OK);
    assert_eq!(fixture.registry.start(id, 4, 100), OK);
    assert_eq!(fixture.registry.remove([1; 16]), OK);
    assert!(fixture.registry.devices().is_empty());
    assert_eq!(LIVE.load(Ordering::SeqCst), 0);
    let retired = fixture.registry.snapshot(id).unwrap();
    assert!(!retired.alive);
    assert!(!retired.running);
    assert_eq!(retired.client_count, 1);
    assert_eq!(fixture.registry.start(id, 5, 200), NOT_FOUND);
    assert_eq!(fixture.registry.stop(id, 4), OK);
    assert_eq!(fixture.registry.client(id, 4, false), OK);
    assert!(fixture.registry.snapshot(id).is_none());
    assert_eq!(LIVE.load(Ordering::SeqCst), 0);
    let replacement = fixture.add(1, 96_000);
    assert_ne!(replacement, id);
    assert!(fixture.registry.snapshot(id).is_none());
    assert_eq!(
        fixture.registry.snapshot(replacement).unwrap().sample_rate,
        96_000
    );
}

#[test]
fn duplicate_client_does_not_reset_running_membership() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    assert_eq!(fixture.registry.client(id, 4, true), OK);
    assert_eq!(fixture.registry.start(id, 4, 100), OK);
    assert_eq!(fixture.registry.client(id, 4, true), INVALID);
    assert_eq!(fixture.registry.stop(id, 4), OK);
    assert!(!fixture.registry.snapshot(id).unwrap().running);
}

#[test]
fn crashed_client_removal_releases_retired_runtime_without_stop_notification() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    assert_eq!(fixture.registry.client(id, 1, true), OK);
    assert_eq!(fixture.registry.start(id, 1, 100), OK);
    assert_eq!(fixture.registry.remove([1; 16]), OK);
    assert_eq!(LIVE.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.registry.client(id, 1, false), OK);
    assert!(fixture.registry.snapshot(id).is_none());
    assert_eq!(LIVE.load(Ordering::SeqCst), 0);
}

/// The device and inode a descriptor refers to, if it is open.
fn file_identity(fd: i32) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    unsafe extern "C" {
        fn fcntl(fd: i32, command: i32) -> i32;
    }
    if unsafe { fcntl(fd, 1) } == -1 {
        return None;
    }
    let file = ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
    file.metadata()
        .ok()
        .map(|metadata| (metadata.dev(), metadata.ino()))
}

#[test]
fn repeated_active_removal_releases_runtime_pending_fd_and_stale_configuration() {
    use std::os::fd::AsRawFd;
    let fixture = Fixture::new();
    for _ in 0..100 {
        let id = fixture.add(1, 48_000);
        assert_eq!(fixture.registry.client(id, 1, true), OK);
        assert_eq!(fixture.registry.start(id, 1, 100), OK);
        assert_eq!(
            fixture
                .registry
                .set_format(id + 1, 48_000, mixrelay_pcm_ring::SampleFormat::Signed24),
            OK
        );
        assert_eq!(fixture.perform(), OK);
        let pending = fixture.fd(1, 8, 2);
        let fd = pending.as_raw_fd();
        let identity = file_identity(fd).unwrap();
        fixture
            .registry
            .upsert([1; 16], "Pending", 96_000, (pending, 24))
            .unwrap();
        let (_, generation) = fixture.pending();
        assert_eq!(fixture.registry.remove([1; 16]), OK);
        assert_eq!(LIVE.load(Ordering::SeqCst), 0);
        // Parallel tests may reuse the descriptor number at once; the file must be gone.
        assert_ne!(file_identity(fd), Some(identity));
        assert_eq!(fixture.registry.set_rate(id, 96_000), NOT_FOUND);
        assert_eq!(
            fixture.registry.perform(id, CONFIG_ACTION, generation),
            NOT_FOUND
        );
        assert!(
            lock(&fixture.registry.pin(id).unwrap().control)
                .pending
                .is_none()
        );
        let mut capture = [0xff_u8; 64 * 3];
        // SAFETY: this retained dead stereo object still advertises packed PCM24.
        assert!(unsafe {
            fixture.registry.io(IoOperation {
                client: 42,
                device: id,
                stream: id + 1,
                render: false,
                frame: 0,
                samples: capture.as_mut_ptr().cast(),
                frames: 32,
            })
        });
        assert_eq!(capture, [0; 64 * 3]);
        assert_eq!(fixture.registry.stop(id, 1), OK);
        assert_eq!(fixture.registry.client(id, 1, false), OK);
        assert!(fixture.registry.snapshot(id).is_none());
    }
}

#[test]
fn rate_requests_report_native_failure_abort_and_success_in_order() {
    unsafe extern "C" fn reject(_: *mut c_void, _: u32) -> bool {
        false
    }
    let mut fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    assert!(fixture.registry.settled_after(0).is_empty());
    let accept = fixture.registry.native.set_rate;
    fixture.registry.native.set_rate = reject;
    assert_eq!(fixture.registry.set_rate(id, 96_000), OK);
    assert_eq!(fixture.perform(), NATIVE_ERROR);
    fixture.registry.native.set_rate = accept;
    assert_eq!(fixture.registry.set_rate(id, 88_200), OK);
    let (_, generation) = fixture.pending();
    assert_eq!(fixture.registry.abort(id, CONFIG_ACTION, generation), OK);
    assert_eq!(fixture.registry.set_rate(id, 96_000), OK);
    assert_eq!(fixture.perform(), OK);
    let settled = fixture.registry.settled_after(0);
    // Every outcome is retained in order, so none hides behind a newer one.
    assert_eq!(
        settled
            .iter()
            .map(|settled| (settled.endpoint, settled.rate, settled.applied))
            .collect::<Vec<_>>(),
        [
            ([1; 16], 96_000, false),
            ([1; 16], 88_200, false),
            ([1; 16], 96_000, true)
        ]
    );
    assert!(
        settled
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    assert_eq!(
        fixture.registry.settled_after(settled[1].sequence),
        settled[2..]
    );
}

fn eventually(condition: impl Fn() -> bool) {
    for _ in 0..10_000 {
        if condition() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("condition did not hold within 10,000 polls");
}

#[test]
fn a_geometry_change_maps_only_its_endpoint_and_no_client_queue() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    // Idle registrations map nothing: a client gets a queue when it starts IO.
    assert_eq!(fixture.registry.client(id, 7, true), OK);
    assert_eq!(PREPARATIONS.load(Ordering::SeqCst), 0);
    fixture
        .registry
        .upsert([1; 16], "Wider", 48_000, (fixture.fd(1, 7, 3), 24))
        .unwrap();
    assert_eq!(fixture.registry.client(id, 8, true), OK);
    // Only the endpoint mapping was prepared, before Perform was requested.
    assert_eq!(PREPARATIONS.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.perform(), OK);
    assert_eq!(PREPARATIONS.load(Ordering::SeqCst), 1);
    assert_eq!(REPLACEMENTS.load(Ordering::SeqCst), 1);
    let snapshot = fixture.registry.snapshot(id).unwrap();
    assert_eq!(
        (snapshot.render_channels, snapshot.capture_channels),
        (7, 3)
    );
    // Each client prepares at its own IO start; one that never starts gets nothing.
    assert_eq!(fixture.registry.start(id, 7, 100), OK);
    assert_eq!(PREPARATIONS.load(Ordering::SeqCst), 2);
}

#[test]
fn a_running_client_left_without_a_queue_is_counted_and_retried_by_the_control_tick() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    assert_eq!(fixture.registry.client(id, 7, true), OK);
    assert_eq!(fixture.registry.start(id, 7, 100), OK);
    assert_eq!(fixture.registry.client(id, 8, true), OK);
    fixture
        .registry
        .upsert([1; 16], "Wider", 48_000, (fixture.fd(1, 7, 3), 24))
        .unwrap();
    let prepared = PREPARATIONS.load(Ordering::SeqCst);
    FAIL_PREPARE_CLIENT.store(true, Ordering::SeqCst);
    // The configuration has committed; the running client's missing queue is reported
    // instead of leaving it silent.
    assert_eq!(fixture.perform(), OK);
    assert_eq!(fixture.registry.unprovisioned(), [([1; 16], 1)]);
    fixture.registry.tick();
    assert_eq!(fixture.registry.unprovisioned(), [([1; 16], 1)]);
    FAIL_PREPARE_CLIENT.store(false, Ordering::SeqCst);
    fixture.registry.tick();
    assert!(fixture.registry.unprovisioned().is_empty());
    // Only the client holding IO was retried; the idle one waits for its StartIO.
    assert_eq!(PREPARATIONS.load(Ordering::SeqCst), prepared + 1);
    fixture.registry.tick();
    assert_eq!(PREPARATIONS.load(Ordering::SeqCst), prepared + 1);
}

#[test]
fn running_clients_are_reprovisioned_off_the_freeze_while_io_is_in_flight() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    assert_eq!(fixture.registry.client(id, 1, true), OK);
    assert_eq!(fixture.registry.start(id, 1, 100), OK);
    let prepared = PREPARATIONS.load(Ordering::SeqCst);
    PAUSE_IO.store(true, Ordering::Release);
    let registry = fixture.registry.as_ref();
    let device = registry.pin(id).unwrap();
    std::thread::scope(|scope| {
        let callback = scope.spawn(|| {
            let mut samples = [0.5_f32; 64];
            // SAFETY: this callback exclusively owns its aligned 32-frame stereo buffer.
            assert!(unsafe {
                registry.io(IoOperation {
                    client: 1,
                    device: id,
                    stream: id + 2,
                    render: true,
                    frame: 0,
                    samples: samples.as_mut_ptr().cast(),
                    frames: 32,
                })
            });
        });
        eventually(|| IO_ENTERED.load(Ordering::Acquire));
        let control = scope.spawn(|| registry.reprovision(id));
        // The queue is mapped while the callback still holds its pin; only the install
        // then waits for the freeze to drain it.
        eventually(|| device.io_pin().is_none());
        assert_eq!(PREPARED_DURING_IO.load(Ordering::SeqCst), 1);
        RELEASE_IO.store(true, Ordering::Release);
        callback.join().unwrap();
        control.join().unwrap();
    });
    assert_eq!(PREPARATIONS.load(Ordering::SeqCst), prepared + 1);
}

#[test]
fn settlements_on_two_devices_at_once_enter_the_queue_in_sequence_order() {
    let fixture = Fixture::new();
    let ids = [fixture.add(1, 48_000), fixture.add(2, 48_000)];
    let registry = fixture.registry.as_ref();
    let start = std::sync::Barrier::new(2);
    let ordered = |registry: &Registry| {
        let sequences: Vec<_> = registry
            .settled_after(0)
            .iter()
            .map(|settled| settled.sequence)
            .collect();
        sequences.windows(2).all(|pair| pair[0] < pair[1])
    };
    // Taking the sequence before the queue lock let [6, 5] enter it; the Driver service
    // then dropped 5 as already seen, losing a prompt rejection.
    let inverted = std::thread::scope(|scope| {
        let writers = ids.map(|id| {
            let start = &start;
            scope.spawn(move || {
                let device = registry.pin(id).unwrap();
                let pending = crate::device::Pending {
                    generation: 1,
                    requested: false,
                    retry_after: 0,
                    failures: 0,
                    attachment: None,
                    name: String::new(),
                    rate: 48_000,
                    formats: [mixrelay_pcm_ring::SampleFormat::Float32; 2],
                };
                start.wait();
                let mut inverted = 0;
                for round in 0..100_000 {
                    registry.settle(&device, &pending, true);
                    if round % 64 == 0 && !ordered(registry) {
                        inverted += 1;
                    }
                }
                inverted
            })
        });
        writers.map(|writer| writer.join().unwrap())
    });
    assert_eq!(inverted, [0, 0]);
    assert!(ordered(registry));
}

#[test]
fn a_format_only_request_and_idle_clients_prepare_nothing() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    assert_eq!(fixture.registry.client(id, 7, true), OK);
    assert_eq!(
        fixture
            .registry
            .set_format(id + 1, 48_000, mixrelay_pcm_ring::SampleFormat::Signed24),
        OK
    );
    assert_eq!(fixture.perform(), OK);
    assert_eq!(PREPARATIONS.load(Ordering::SeqCst), 0);
    // Each frozen client change and the commit released what the freeze detached.
    assert_eq!(fixture.registry.client(id, 7, false), OK);
    assert_eq!(RELEASES.load(Ordering::SeqCst), 3);
}

#[test]
fn queue_and_replacement_preparation_run_alongside_in_flight_io() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    assert_eq!(fixture.registry.client(id, 1, true), OK);
    assert_eq!(fixture.registry.start(id, 1, 100), OK);
    let descriptor = fixture.fd(1, 7, 3);
    PAUSE_IO.store(true, Ordering::Release);
    let registry = fixture.registry.as_ref();
    let device = registry.pin(id).unwrap();
    std::thread::scope(|scope| {
        let callback = scope.spawn(|| {
            let mut samples = [0.5_f32; 64];
            // SAFETY: this callback exclusively owns its aligned 32-frame stereo buffer.
            assert!(unsafe {
                registry.io(IoOperation {
                    client: 1,
                    device: id,
                    stream: id + 2,
                    render: true,
                    frame: 0,
                    samples: samples.as_mut_ptr().cast(),
                    frames: 32,
                })
            });
        });
        eventually(|| IO_ENTERED.load(Ordering::Acquire));
        let control = scope.spawn(|| {
            registry
                .upsert([1; 16], "Wider", 48_000, (descriptor, 24))
                .unwrap();
            assert_eq!(registry.client(id, 2, true), OK);
        });
        // Both allocations finish while the callback still holds its pin; only the
        // client's publication then waits for the freeze to drain it.
        eventually(|| device.io_pin().is_none());
        assert_eq!(PREPARED_DURING_IO.load(Ordering::SeqCst), 2);
        RELEASE_IO.store(true, Ordering::Release);
        callback.join().unwrap();
        control.join().unwrap();
    });
    // The first start mapped its client's queue; the endpoint mapping and the client that
    // joined the running device were each prepared alongside the in-flight callback.
    assert_eq!(PREPARATIONS.load(Ordering::SeqCst), 3);
    drop(device);
    assert_eq!(fixture.perform(), OK);
    assert_eq!(REPLACEMENTS.load(Ordering::SeqCst), 1);
}

#[test]
fn active_callback_drains_before_removal_releases_its_runtime() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    assert_eq!(fixture.registry.client(id, 1, true), OK);
    assert_eq!(fixture.registry.start(id, 1, 100), OK);
    PAUSE_IO.store(true, Ordering::Release);
    let registry = fixture.registry.as_ref();
    let device = registry.pin(id).unwrap();
    let removed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let callback = scope.spawn(|| {
            let mut samples = [0.5_f32; 64];
            // SAFETY: this callback exclusively owns its aligned 32-frame stereo buffer.
            assert!(unsafe {
                registry.io(IoOperation {
                    client: 42,
                    device: id,
                    stream: id + 2,
                    render: true,
                    frame: 0,
                    samples: samples.as_mut_ptr().cast(),
                    frames: 32,
                })
            });
        });
        while !IO_ENTERED.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        let removal = scope.spawn(|| {
            assert_eq!(registry.remove([1; 16]), OK);
            removed.store(true, Ordering::Release);
        });
        while device.io_pin().is_some() {
            std::thread::yield_now();
        }
        let premature_removal = removed.load(Ordering::Acquire);
        let live = LIVE.load(Ordering::SeqCst);
        RELEASE_IO.store(true, Ordering::Release);
        callback.join().unwrap();
        removal.join().unwrap();
        assert!(!premature_removal);
        assert_eq!(live, 1);
    });
    assert!(removed.load(Ordering::Acquire));
    assert_eq!(LIVE.load(Ordering::SeqCst), 0);
    drop(device);
    assert_eq!(registry.stop(id, 1), OK);
    assert_eq!(registry.client(id, 1, false), OK);
    assert!(registry.snapshot(id).is_none());
}

#[test]
fn reconnect_retains_live_clock_and_native_rate_failure_is_atomic() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    fixture.registry.client(id, 4, true);
    fixture.registry.start(id, 4, 100);
    assert_eq!(fixture.registry.set_rate(id, 192_000), OK);
    assert_eq!(fixture.perform(), OK);
    fixture
        .registry
        .upsert([1; 16], "Reconnect", 48_000, (fixture.fd(1, 7, 3), 24))
        .unwrap();
    FAIL_REPLACE.store(true, Ordering::SeqCst);
    assert_eq!(fixture.perform(), NATIVE_ERROR);
    let snapshot = fixture.registry.snapshot(id).unwrap();
    assert_eq!(
        (snapshot.render_channels, snapshot.sample_rate),
        (2, 192_000)
    );
    FAIL_REPLACE.store(false, Ordering::SeqCst);
    fixture
        .registry
        .upsert([1; 16], "Reconnect", 48_000, (fixture.fd(1, 7, 3), 24))
        .unwrap();
    assert_eq!(fixture.perform(), OK);
    let mut timestamp = Timestamp::default();
    assert!(fixture.registry.timestamp(id, 200, &mut timestamp));
    assert_eq!(timestamp.sample_time, 192_000);
    assert!(fixture.registry.snapshot(id).unwrap().running);
}

#[test]
fn failed_capture_pcm_preparation_returns_without_reserving_native_output_storage() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    for stream in [id + 1, id + 2] {
        assert_eq!(
            fixture
                .registry
                .set_format(stream, 96_000, mixrelay_pcm_ring::SampleFormat::Signed24),
            OK
        );
    }
    let device = fixture.registry.pin(id).unwrap();
    let control = lock(&device.control);
    let pending = control.pending.as_ref().unwrap();
    let mut attempted = 0;
    let allocation = allocation_counter::measure(|| {
        let result = device.prepare_scratch_with(pending, |_| {
            attempted += 1;
            mixrelay_audio_io::pcm_buffer::zeroed(usize::MAX)
        });
        assert!(result.is_err());
    });
    assert_eq!(attempted, 1);
    assert_eq!((allocation.count_current, allocation.bytes_current), (0, 0));
}

#[test]
fn failed_pcm_reservation_preserves_live_format_mapping_clock_and_audio() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    assert_eq!(fixture.registry.client(id, 4, true), OK);
    assert_eq!(fixture.registry.start(id, 4, 100), OK);
    for stream in [id + 1, id + 2] {
        assert_eq!(
            fixture
                .registry
                .set_format(stream, 96_000, mixrelay_pcm_ring::SampleFormat::Signed24),
            OK
        );
    }
    fixture
        .registry
        .upsert([1; 16], "Replacement", 96_000, (fixture.fd(1, 4, 4), 24))
        .unwrap();
    let (_, generation) = fixture.pending();
    let mut before = Timestamp::default();
    assert!(fixture.registry.timestamp(id, 200, &mut before));
    assert_eq!(
        fixture
            .registry
            .commit(id, CONFIG_ACTION, generation, |device, pending| {
                device.prepare_scratch_with(pending, |_| {
                    mixrelay_audio_io::pcm_buffer::zeroed(usize::MAX)
                })
            }),
        CAPACITY
    );
    let snapshot = fixture.registry.snapshot(id).unwrap();
    assert_eq!(
        (
            snapshot.sample_rate,
            snapshot.render_channels,
            snapshot.capture_channels
        ),
        (48_000, 2, 2)
    );
    assert_eq!(
        snapshot.capture_format,
        mixrelay_pcm_ring::SampleFormat::Float32 as u32
    );
    assert_eq!(
        snapshot.render_format,
        mixrelay_pcm_ring::SampleFormat::Float32 as u32
    );
    assert!(snapshot.running);
    assert!(fixture.registry.attachment_failed([1; 16]));
    assert_eq!(REPLACEMENTS.load(Ordering::SeqCst), 0);
    let mut after = Timestamp::default();
    assert!(fixture.registry.timestamp(id, 300, &mut after));
    assert_eq!(before.seed, after.seed);
    for render in [true, false] {
        let mut samples = if render { [0.25_f32; 4] } else { [0.0; 4] };
        // SAFETY: the unchanged active format owns two complete Float32 stereo frames.
        assert!(unsafe {
            fixture.registry.io(IoOperation {
                client: 42,
                device: id,
                stream: id + if render { 2 } else { 1 },
                render,
                frame: 0,
                samples: samples.as_mut_ptr().cast(),
                frames: 2,
            })
        });
        assert_eq!(samples.map(f32::to_bits), [0.25_f32; 4].map(f32::to_bits));
    }
}

#[test]
fn all_product_rates_are_independent_and_transport_identity_is_validated() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    for &rate in crate::formats::SAMPLE_RATES {
        assert_eq!(fixture.registry.set_rate(id, rate), OK);
        assert_eq!(fixture.perform(), OK);
        assert_eq!(fixture.registry.snapshot(id).unwrap().sample_rate, rate);
    }
    assert_eq!(fixture.registry.set_rate(id, 384_000), INVALID);
    assert_eq!(
        fixture
            .registry
            .upsert([2; 16], "Wrong identity", 48_000, (fixture.fd(3, 2, 2), 24)),
        Err(INVALID)
    );
    assert_eq!(fixture.registry.devices(), [id]);
}

#[test]
fn property_abi_exposes_per_device_stream_shape_and_partial_lists() {
    use crate::properties::{Address, mr_hal_core_property_get, mr_hal_core_property_size};
    let fixture = Fixture::new();
    let first = fixture.add(1, 48_000);
    let second = fixture.add(2, 96_000);
    let registry = (&raw const *fixture.registry).cast_mut();
    let list = Address {
        selector: u32::from_be_bytes(*b"dev#"),
        scope: u32::from_be_bytes(*b"glob"),
        element: 0,
    };
    let mut size = 0;
    assert_eq!(
        unsafe { mr_hal_core_property_size(registry, 1, &raw const list, &raw mut size) },
        0
    );
    assert_eq!(size, 8);
    let mut one = 0_u32;
    assert_eq!(
        unsafe {
            mr_hal_core_property_get(
                registry,
                1,
                &raw const list,
                0,
                std::ptr::null(),
                4,
                &raw mut size,
                (&raw mut one).cast(),
                0,
            )
        },
        0
    );
    assert_eq!((size, one), (4, first));
    let format = Address {
        selector: u32::from_be_bytes(*b"sfmt"),
        ..list
    };
    let bytes = fixture.property::<40>(second + 2, &format);
    assert_eq!(
        f64::from_ne_bytes(bytes[..8].try_into().unwrap()).to_bits(),
        96_000_f64.to_bits()
    );
    assert_eq!(u32::from_ne_bytes(bytes[28..32].try_into().unwrap()), 2);
    fixture
        .registry
        .upsert([2; 16], "Discrete", 96_000, (fixture.fd(2, 7, 33), 24))
        .unwrap();
    assert_eq!(fixture.perform(), OK);
    let bytes = fixture.property::<40>(second + 2, &format);
    assert_eq!(u32::from_ne_bytes(bytes[28..32].try_into().unwrap()), 7);
    let layout = Address {
        selector: u32::from_be_bytes(*b"srnd"),
        scope: u32::from_be_bytes(*b"inpt"),
        element: 0,
    };
    assert_eq!(
        unsafe { mr_hal_core_property_size(registry, second, &raw const layout, &raw mut size) },
        0
    );
    assert_eq!(size, 12 + 33 * 20);
}

#[test]
fn property_abi_validates_formats_and_cf_string_ownership() {
    use crate::properties::{Address, mr_hal_core_property_get, mr_hal_core_property_set};
    unsafe extern "C" {
        fn CFRelease(value: *const c_void);
    }
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let registry = (&raw const *fixture.registry).cast_mut();
    let property = Address {
        selector: u32::from_be_bytes(*b"nsrt"),
        scope: u32::from_be_bytes(*b"glob"),
        element: 0,
    };
    let invalid = 44_100.5_f64;
    assert_ne!(
        unsafe {
            mr_hal_core_property_set(
                registry,
                id,
                &raw const property,
                8,
                (&raw const invalid).cast(),
            )
        },
        0
    );
    assert!(lock(&fixture.host.requests).is_empty());
    let requested = 192_000_f64;
    assert_eq!(
        unsafe {
            mr_hal_core_property_set(
                registry,
                id,
                &raw const property,
                8,
                (&raw const requested).cast(),
            )
        },
        0
    );
    assert_eq!(fixture.perform(), OK);
    let property = Address {
        selector: u32::from_be_bytes(*b"lnam"),
        ..property
    };
    let mut name: *const c_void = std::ptr::null();
    let mut written = 0;
    assert_eq!(
        unsafe {
            mr_hal_core_property_get(
                registry,
                id,
                &raw const property,
                0,
                std::ptr::null(),
                8,
                &raw mut written,
                (&raw mut name).cast(),
                0,
            )
        },
        0
    );
    assert!(!name.is_null());
    assert_eq!(written, 8);
    unsafe { CFRelease(name) };
}

#[test]
fn ffi_rejects_foreign_transport_and_closes_consumed_descriptor() {
    use crate::abi::mr_hal_core_upsert;
    use std::os::fd::IntoRawFd;
    let fixture = Fixture::new();
    let fd = fixture.fd(2, 2, 2).into_raw_fd();
    let file = file_identity(fd).unwrap();
    let mut output = 99;
    assert_eq!(
        unsafe {
            mr_hal_core_upsert(
                (&raw const *fixture.registry).cast_mut(),
                &[1; 16],
                c"Foreign".as_ptr(),
                48_000,
                fd,
                24,
                &raw mut output,
            )
        },
        INVALID
    );
    assert_ne!(file_identity(fd), Some(file));
    assert_eq!(output, 99);
    assert!(fixture.registry.devices().is_empty());
}

#[test]
fn io_device_query_completes_while_metadata_control_is_locked() {
    use crate::abi::mr_hal_core_device_alive;
    use std::sync::mpsc;
    use std::time::Duration;

    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let registry = fixture.registry.as_ref();
    let pin = registry.pin(id).unwrap();
    let control = lock(&pin.control);
    let (send, receive) = mpsc::channel();
    let response = std::thread::scope(|scope| {
        scope.spawn(|| {
            send.send(unsafe { mr_hal_core_device_alive(registry, id) })
                .unwrap();
        });
        let response = receive.recv_timeout(Duration::from_secs(1));
        // Release before joining even on regression, so a failure cannot hang
        // the test runner behind the deliberately held control mutex.
        drop(control);
        response
    });
    assert_eq!(response, Ok(true));
    drop(pin);
    assert!(!unsafe { mr_hal_core_device_alive(registry, id + 1) });
    assert!(!unsafe { mr_hal_core_device_alive(registry, id + 2) });
    assert!(!unsafe { mr_hal_core_device_alive(std::ptr::null(), id) });
    assert_eq!(registry.remove([1; 16]), OK);
    assert!(!unsafe { mr_hal_core_device_alive(registry, id) });
}

#[test]
fn callback_readers_drain_before_retired_runtime_is_freed() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    assert_eq!(fixture.registry.client(id, 10, true), OK);
    assert_eq!(fixture.registry.start(id, 10, 1), OK);
    let registry = fixture.registry.as_ref();
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            let mut timestamp = Timestamp::default();
            while !stop.load(Ordering::Acquire) {
                registry.timestamp(id, 100, &mut timestamp);
            }
        });
        for _ in 0..100 {
            assert_eq!(registry.set_output_volume(id + 3, 0.5), OK);
            assert_eq!(registry.set_output_mute(id + 4, true), OK);
            registry.set_rate(id, 96_000);
            fixture.perform();
        }
        assert_eq!(registry.remove([1; 16]), OK);
        assert_eq!(registry.stop(id, 10), OK);
        assert_eq!(registry.client(id, 10, false), OK);
        registry.tick();
        stop.store(true, Ordering::Release);
        reader.join().unwrap();
    });
    assert_eq!(LIVE.load(Ordering::SeqCst), 0);
    assert!(registry.snapshot(id).is_none());
    assert!(registry.output_control(id + 3).is_none());
    assert_eq!(registry.set_output_mute(id + 4, false), NOT_FOUND);
}

/// Installed behaviour under the driver sandbox, where no process can be inspected: the
/// engine renders into its own endpoint and must still see its reported latency, while any
/// other rendering process zeroes it.
#[test]
fn a_sandboxed_engine_rendering_its_own_endpoint_keeps_the_reported_latency() {
    use mixrelay_macos_transport::control_inventory::InputLatency;
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let registry = &fixture.registry;
    let registry_ptr = (&raw const **registry).cast_mut();
    let identity = |client: u32, pid: i32, add: bool| {
        // SAFETY: the fixture keeps the registry alive; a null bundle is a bare process.
        unsafe {
            crate::abi::mr_hal_core_client_identity(
                registry_ptr,
                id,
                client,
                pid,
                std::ptr::null(),
                add,
            )
        }
    };
    registry.publish_latencies(&[InputLatency {
        endpoint_id: [1; 16],
        rate: 48_000,
        engine_pid: 10,
        lease: 20,
        revision: 30,
        frames: Some(751),
    }]);
    assert_eq!(identity(1, 10, true), OK);
    assert_eq!(registry.start(id, 1, 0), OK);
    registry.render_client(id, 1);
    registry.tick();
    assert_eq!(
        registry.pin(id).unwrap().input_latency.effective(48_000),
        751
    );
    assert_eq!(identity(2, 11, true), OK);
    assert_eq!(registry.start(id, 2, 0), OK);
    registry.render_client(id, 2);
    registry.tick();
    assert_eq!(registry.pin(id).unwrap().input_latency.effective(48_000), 0);
    assert_eq!(registry.stop(id, 2), OK);
    assert_eq!(identity(2, 11, false), OK);
    assert_eq!(registry.stop(id, 1), OK);
    assert_eq!(identity(1, 10, false), OK);
}

/// Installed behaviour under the driver sandbox: an app on the USB policy is presented as USB
/// from the bundle coreaudiod names when it registers, and an unregistered or other app is not.
#[test]
fn a_sandboxed_registered_app_on_the_policy_is_presented_as_usb() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let registry = &fixture.registry;
    let registry_ptr = (&raw const **registry).cast_mut();
    registry.transport_policy(vec!["com.tencent.tencentmeeting".into()]);
    let usb = u32::from_be_bytes(*b"usb ");
    let read = |pid: i32| {
        registry.transport.get_until(
            pid,
            registry.transport.deadline(crate::transport::QUERY_BUDGET),
        )
    };
    for (client, pid, bundle) in [
        (1, 4242, c"com.tencent.tencentmeeting"),
        (2, 4343, c"com.example.Other"),
    ] {
        // SAFETY: the fixture keeps the registry alive; the bundle outlives the call.
        let status = unsafe {
            crate::abi::mr_hal_core_client_identity(
                registry_ptr,
                id,
                client,
                pid,
                bundle.as_ptr(),
                true,
            )
        };
        assert_eq!(status, OK);
    }
    assert_eq!(read(4242), usb);
    assert_eq!(read(4343), crate::transport::VIRTUAL);
    assert_eq!(read(4444), crate::transport::VIRTUAL);
    // SAFETY: as above; removal names no bundle.
    let removed = unsafe {
        crate::abi::mr_hal_core_client_identity(registry_ptr, id, 1, 4242, std::ptr::null(), false)
    };
    assert_eq!(removed, OK);
    assert_eq!(read(4242), crate::transport::VIRTUAL);
}

#[test]
fn runtime_input_latency_fences_stale_leases_and_notifies_after_unlock() {
    use mixrelay_macos_transport::control_inventory::InputLatency;
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let entry = InputLatency {
        endpoint_id: [1; 16],
        rate: 48_000,
        engine_pid: 10,
        lease: 20,
        revision: 30,
        frames: Some(300),
    };
    let registry = &fixture.registry;
    registry.publish_latencies(&[entry]);
    assert_eq!(
        registry.pin(id).unwrap().input_latency.effective(48_000),
        300
    );
    registry.publish_latencies(&[InputLatency {
        lease: 19,
        frames: Some(400),
        ..entry
    }]);
    registry.publish_latencies(&[InputLatency {
        revision: 29,
        frames: Some(400),
        ..entry
    }]);
    assert_eq!(
        registry.pin(id).unwrap().input_latency.effective(48_000),
        300
    );
    assert_eq!(
        lock(&fixture.host.changes)
            .iter()
            .filter(|(_, change)| *change == crate::abi::INPUT_LATENCY)
            .count(),
        1
    );
    assert_eq!(registry.client(id, 1, true), OK);
    registry.client_pid(id, 1, 11);
    assert_eq!(registry.start(id, 1, 0), OK);
    registry.render_client(id, 1);
    registry.tick();
    assert_eq!(registry.pin(id).unwrap().input_latency.effective(48_000), 0);
    assert_eq!(registry.stop(id, 1), OK);
    registry.tick();
    assert_eq!(
        registry.pin(id).unwrap().input_latency.effective(48_000),
        300
    );
    registry.clear_latencies();
    assert_eq!(registry.pin(id).unwrap().input_latency.effective(48_000), 0);
    registry.publish_latencies(&[]);
}
