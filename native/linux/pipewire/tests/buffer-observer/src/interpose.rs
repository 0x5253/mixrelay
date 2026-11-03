use crate::{
    directions::Directions,
    journal::Journal,
    record::{Operation, Record, monotonic_ns},
};
use pipewire::sys;
use std::{
    ffi::c_char,
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

type Open = unsafe extern "C" fn(
    *mut sys::pw_core,
    *const c_char,
    *mut sys::pw_properties,
) -> *mut sys::pw_stream;
type Close = unsafe extern "C" fn(*mut sys::pw_stream);
type Dequeue = unsafe extern "C" fn(*mut sys::pw_stream) -> *mut sys::pw_buffer;
type Queue = unsafe extern "C" fn(*mut sys::pw_stream, *mut sys::pw_buffer) -> i32;
type Trigger = unsafe extern "C" fn(*mut sys::pw_stream) -> i32;
type Connect = unsafe extern "C" fn(
    *mut sys::pw_stream,
    pipewire::spa::sys::spa_direction,
    u32,
    sys::pw_stream_flags,
    *mut *const pipewire::spa::sys::spa_pod,
    u32,
) -> i32;

struct Functions {
    open: Open,
    close: Close,
    dequeue: Dequeue,
    queue: Queue,
    connect: Connect,
    trigger: Trigger,
}
static REAL: OnceLock<Functions> = OnceLock::new();
static OUTPUT: OnceLock<PathBuf> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);
static LIVE_STREAMS: AtomicUsize = AtomicUsize::new(0);
static RECORDS: Journal<131_072> = Journal::new();
static DIRECTIONS: Directions<32> = Directions::new();
static UNTRACKED_STREAMS: AtomicUsize = AtomicUsize::new(0);

fn resolve_symbols() -> Option<Functions> {
    let names = [
        c"pw_stream_new",
        c"pw_stream_destroy",
        c"pw_stream_dequeue_buffer",
        c"pw_stream_queue_buffer",
        c"pw_stream_connect",
        c"pw_stream_trigger_process",
    ];
    let [open, close, dequeue, queue, connect, trigger] = names.map(|name| {
        // SAFETY: immutable, terminated symbol name. Resolve only during loader
        // initialization, never lazily on the first audio callback.
        unsafe { libc::dlsym(libc::RTLD_NEXT, name.as_ptr()) }
    });
    if [open, close, dequeue, queue, connect, trigger]
        .iter()
        .any(|pointer| pointer.is_null())
    {
        return None;
    }
    // SAFETY: exact public PipeWire C function signatures from the same linked
    // ABI. RTLD_NEXT resolves past this observer, preventing recursive hooks.
    Some(unsafe {
        Functions {
            open: std::mem::transmute::<*mut libc::c_void, Open>(open),
            close: std::mem::transmute::<*mut libc::c_void, Close>(close),
            dequeue: std::mem::transmute::<*mut libc::c_void, Dequeue>(dequeue),
            queue: std::mem::transmute::<*mut libc::c_void, Queue>(queue),
            connect: std::mem::transmute::<*mut libc::c_void, Connect>(connect),
            trigger: std::mem::transmute::<*mut libc::c_void, Trigger>(trigger),
        }
    })
}

fn real() -> &'static Functions {
    // A loader-order/symbol failure is a failed private test, never an invented
    // successful transfer or an unwind through the C callback boundary.
    REAL.get().unwrap_or_else(|| std::process::abort())
}

extern "C" fn initialize() {
    let Some(functions) = resolve_symbols() else {
        std::process::abort();
    };
    if REAL.set(functions).is_err() {
        std::process::abort();
    }
    let Ok(executable) = std::env::current_exe() else {
        std::process::abort();
    };
    let name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    // The test runner's inventory/ps/kill children inherit LD_PRELOAD, but are
    // not audio clients and do not allocate or publish an observer journal.
    if !(name.starts_with("mixrelay_service-")
        || name.starts_with("mixrelay_linux_driver-")
        || matches!(name, "application-fixture" | "pw-cat" | "pw-record"))
    {
        return;
    }
    let fixture = std::env::var_os("MIXRELAY_LAB_APPLICATION_FIXTURE").map(PathBuf::from);
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let Some(directory) = fixture.as_ref().and_then(|path| path.parent()) else {
        std::process::abort();
    };
    if !directory.is_absolute()
        || !directory
            .to_string_lossy()
            .contains("/.lab/mixrelay-integration-")
        || runtime.as_deref() != Some(directory.join("runtime").as_path())
    {
        std::process::abort();
    }
    if OUTPUT
        .set(directory.join(format!("buffer-observer-{}.json", std::process::id())))
        .is_err()
    {
        std::process::abort();
    }
    RECORDS.prefault();
    ENABLED.store(true, Ordering::Release);
}

extern "C" fn finalize() {
    ENABLED.store(false, Ordering::Release);
    let Some(path) = OUTPUT.get() else {
        return;
    };
    let save = || -> Result<(), Box<dyn std::error::Error>> {
        let journal = RECORDS.snapshot();
        let evidence = serde_json::json!({
            "processId": std::process::id(), "liveStreamsAtExit": LIVE_STREAMS.load(Ordering::Acquire),
            "untrackedStreams": UNTRACKED_STREAMS.load(Ordering::Acquire),
            "scope": "native handoff geometry and bounded PCM byte edges; payload hashes qualify only little-endian f32 stereo, at most 8192 frames per buffer",
            "timestampSemantics": {
                "Open": "after native stream creation",
                "Dequeue": "after native dequeue returns",
                "Queue": "before native queue call; result recorded after return",
                "Close": "after native destroy and callback drain return",
                "ProcessEnter": "entry hook; pair matching stream and native_thread_id",
                "ProcessExit": "exit hook; elapsed includes observer work and preemption",
                "native_thread_cpu_ns": "cumulative guest-accounted CLOCK_THREAD_CPUTIME_ID at process edges only; pair matching native_thread_id; includes observer work; kernel IRQ/steal accounting determines whether host vCPU pauses are excluded",
                "TriggerEnter": "before native trigger call; elapsed to ProcessEnter includes observer work, native dispatch and preemption, not pure wake time",
                "TriggerExit": "after native trigger returns; result preserved, no native clock metadata"
            },
            "journal": journal,
        });
        let mut writer = BufWriter::new(File::create_new(path)?);
        serde_json::to_writer(&mut writer, &evidence)?;
        writer.flush()?;
        Ok(())
    };
    if save().is_err() {
        std::process::abort();
    }
}

// Linux ELF loader hooks; not compiled into unit-test binaries or other OSes.
#[used]
#[unsafe(link_section = ".init_array")]
static INITIALIZE: extern "C" fn() = initialize;
#[used]
#[unsafe(link_section = ".fini_array")]
static FINALIZE: extern "C" fn() = finalize;

/// Records a private-lab process boundary without accessing any PCM or buffer descriptor.
/// `phase` is 1 at entry and 2 at exit; other phases produce no record.
/// # Safety
/// `stream` must be live inside its serialized native process callback. The
/// callback owner must drain before stream destruction. A null stream is ignored.
#[unsafe(no_mangle)]
unsafe extern "C" fn mr_pw_observe_process(stream: *mut sys::pw_stream, phase: u32) {
    if !ENABLED.load(Ordering::Acquire) || stream.is_null() {
        return;
    }
    let Some(operation) = Operation::process_phase(phase) else {
        return;
    };
    // SAFETY: the diagnostic callback owner retains its live stream through this
    // hook; null buffer/false forbid buffer, chunk, metadata and PCM access.
    RECORDS.push(&unsafe { Record::snapshot(operation, stream, std::ptr::null_mut(), false) });
}

/// Observes creation without changing native properties or callback dispatch.
/// # Safety
/// Arguments must satisfy `pw_stream_new`'s C ABI; native takes properties ownership.
#[unsafe(no_mangle)]
unsafe extern "C" fn pw_stream_new(
    core: *mut sys::pw_core,
    name: *const c_char,
    properties: *mut sys::pw_properties,
) -> *mut sys::pw_stream {
    // SAFETY: forward exactly once to the resolved native function, unchanged.
    let stream = unsafe { (real().open)(core, name, properties) };
    if !stream.is_null() && ENABLED.load(Ordering::Acquire) {
        LIVE_STREAMS.fetch_add(1, Ordering::Relaxed);
        if !DIRECTIONS.open(stream as usize) {
            UNTRACKED_STREAMS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: successful creation owns this live stream; there is no buffer.
        RECORDS.push(&unsafe {
            Record::snapshot(Operation::Open, stream, std::ptr::null_mut(), false)
        });
    }
    stream
}

/// Observes a received buffer before any application reads or rewrites its PCM.
/// # Safety
/// The stream must be live and the caller must satisfy the native dequeue contract.
#[unsafe(no_mangle)]
unsafe extern "C" fn pw_stream_dequeue_buffer(stream: *mut sys::pw_stream) -> *mut sys::pw_buffer {
    // SAFETY: original native operation, unchanged ownership/result.
    let buffer = unsafe { (real().dequeue)(stream) };
    if ENABLED.load(Ordering::Acquire) {
        // SAFETY: input dequeue grants initialized PCM until queue. Output and
        // unknown stream directions never inspect possibly uninitialized PCM.
        RECORDS.push(&unsafe {
            Record::snapshot(
                Operation::Dequeue,
                stream,
                buffer,
                DIRECTIONS.input(stream as usize),
            )
        });
    }
    buffer
}

/// Copies evidence before handing the buffer back, preserving native return status.
/// # Safety
/// Stream/buffer must be a valid native queue pair, exclusively owned by this caller.
#[unsafe(no_mangle)]
unsafe extern "C" fn pw_stream_queue_buffer(
    stream: *mut sys::pw_stream,
    buffer: *mut sys::pw_buffer,
) -> i32 {
    let evidence = ENABLED.load(Ordering::Acquire).then(|| {
        // SAFETY: ownership has not been returned yet; only value copies escape.
        unsafe { Record::snapshot(Operation::Queue, stream, buffer, true) }
    });
    // SAFETY: forward once, then never read the returned native buffer again.
    let result = unsafe { (real().queue)(stream, buffer) };
    if let Some(mut evidence) = evidence {
        evidence.result = result;
        RECORDS.push(&evidence);
    }
    result
}

/// Records trigger request and return without querying a stream from a foreign thread.
/// # Safety
/// The live stream must satisfy `pw_stream_trigger_process`'s native calling contract.
#[unsafe(no_mangle)]
unsafe extern "C" fn pw_stream_trigger_process(stream: *mut sys::pw_stream) -> i32 {
    let observe = ENABLED.load(Ordering::Acquire);
    if observe {
        RECORDS.push(&Record::trigger(Operation::TriggerEnter, stream as usize));
    }
    // SAFETY: forward exactly once to the loader-resolved native function; the
    // observer neither dereferences the stream nor queries foreign-thread metadata.
    let result = unsafe { (real().trigger)(stream) };
    if observe {
        let mut evidence = Record::trigger(Operation::TriggerExit, stream as usize);
        evidence.result = result;
        RECORDS.push(&evidence);
    }
    result
}

/// Records native drain completion before the stream pointer can be recycled.
/// # Safety
/// The pointer must be a live, uniquely destroyed stream, never already released.
#[unsafe(no_mangle)]
unsafe extern "C" fn pw_stream_destroy(stream: *mut sys::pw_stream) {
    let evidence = ENABLED.load(Ordering::Acquire).then(|| {
        // SAFETY: stream is still live here; no native access occurs after destroy.
        unsafe { Record::snapshot(Operation::Close, stream, std::ptr::null_mut(), false) }
    });
    // SAFETY: original destruction must drain before the caller releases callbacks.
    unsafe { (real().close)(stream) };
    if let Some(mut evidence) = evidence {
        DIRECTIONS.close(stream as usize);
        evidence.monotonic_ns = monotonic_ns();
        LIVE_STREAMS.fetch_sub(1, Ordering::Release);
        RECORDS.push(&evidence);
    }
}

/// Publishes the native direction before connect can activate process callbacks.
/// # Safety
/// Arguments obey `pw_stream_connect`'s ABI, including the `n_params`-sized pod array.
#[unsafe(no_mangle)]
unsafe extern "C" fn pw_stream_connect(
    stream: *mut sys::pw_stream,
    direction: pipewire::spa::sys::spa_direction,
    target_id: u32,
    flags: sys::pw_stream_flags,
    params: *mut *const pipewire::spa::sys::spa_pod,
    n_params: u32,
) -> i32 {
    if ENABLED.load(Ordering::Acquire) {
        DIRECTIONS.connect(
            stream as usize,
            direction == pipewire::spa::sys::SPA_DIRECTION_INPUT,
        );
    }
    // SAFETY: preserve the native six-argument ABI, flags, pods and return code.
    unsafe { (real().connect)(stream, direction, target_id, flags, params, n_params) }
}
