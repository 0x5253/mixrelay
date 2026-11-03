use pipewire::{spa, sys};
use serde::Serialize;
use std::{ffi::c_char, mem::MaybeUninit};

#[derive(Clone, Copy, Default, Serialize)]
pub(crate) struct Payload {
    pub(crate) bytes: u32,
    stride: i32,
    valid: bool,
    first: [u32; 2],
    last: [u32; 2],
    // Fixed-width hex avoids JSON consumers rounding a u64 checksum.
    hash: PayloadHash,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct PayloadHash(u64);

impl Serialize for PayloadHash {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{:016x}", self.0))
    }
}

impl Payload {
    fn measure(bytes: &[u8], stride: i32) -> Self {
        let mut result = Self {
            bytes: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
            stride,
            ..Self::default()
        };
        if bytes.is_empty()
            || bytes.len() > 65_536
            || !bytes.len().is_multiple_of(8)
            || !matches!(stride, 0 | 8)
        {
            return result;
        }
        // A non-empty multiple of eight bytes holds at least one stereo frame.
        let (words, _) = bytes.as_chunks::<4>();
        let ([first_left, first_right, ..], [.., last_left, last_right]) = (words, words) else {
            return result;
        };
        result.valid = true;
        result.first = [
            u32::from_ne_bytes(*first_left),
            u32::from_ne_bytes(*first_right),
        ];
        result.last = [
            u32::from_ne_bytes(*last_left),
            u32::from_ne_bytes(*last_right),
        ];
        result.hash = PayloadHash(bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
        }));
        result
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) enum Operation {
    Open,
    Dequeue,
    Queue,
    Close,
    ProcessEnter,
    ProcessExit,
    TriggerEnter,
    TriggerExit,
}

impl Operation {
    pub(crate) fn process_phase(phase: u32) -> Option<Self> {
        match phase {
            1 => Some(Self::ProcessEnter),
            2 => Some(Self::ProcessExit),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Default, Serialize)]
struct Header {
    flags: u32,
    pts: i64,
    sequence: u64,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize)]
struct ByteEdges {
    count: u8,
    first: [u8; 8],
    last: [u8; 8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
struct ChunkGeometry {
    offset: u32,
    size: u32,
    stride: i32,
    bounds_valid: bool,
    edges: Option<ByteEdges>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
struct BufferGeometry {
    requested: u64,
    maxsize: Option<u32>,
    chunk: Option<ChunkGeometry>,
}

#[derive(Clone, Copy, Serialize)]
pub(crate) struct Record {
    pub(crate) operation: Operation,
    pub(crate) stream: usize,
    pub(crate) monotonic_ns: Option<u64>,
    native_thread_id: Option<u64>,
    // Cumulative CPU execution of this thread, sampled only at callback edges.
    // Pair with the same thread ID. Guest accounting may charge host vCPU pauses
    // or interrupt time when the kernel does not separately account for them.
    native_thread_cpu_ns: Option<u64>,
    #[serde(serialize_with = "serialize_name")]
    name: [u8; 32],
    pub(crate) result: i32,
    buffer_present: bool,
    native_ticks: u64,
    native_report_ns: i64,
    rate_num: u32,
    rate_denom: u32,
    native_time_result: i32,
    buffered_frames: Option<u64>,
    queued_buffers: Option<u32>,
    avail_buffers: Option<u32>,
    buffer_time_ns: Option<u64>,
    header: Option<Header>,
    geometry: Option<BufferGeometry>,
    pub(crate) payload: Payload,
}

fn serialize_name<S: serde::Serializer>(name: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
    let end = name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name.len());
    serializer.serialize_str(&String::from_utf8_lossy(&name[..end]))
}

pub(crate) fn monotonic_ns() -> Option<u64> {
    clock_ns(libc::CLOCK_MONOTONIC)
}

fn clock_ns(clock: libc::clockid_t) -> Option<u64> {
    let mut value = MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: complete output storage; querying a clock changes no clock or
    // scheduling policy. Read the initialized fields only after success.
    if unsafe { libc::clock_gettime(clock, value.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: successful clock_gettime initialized both native timespec fields.
    let value = unsafe { value.assume_init() };
    u64::try_from(value.tv_sec)
        .ok()?
        .checked_mul(1_000_000_000)?
        .checked_add(u64::try_from(value.tv_nsec).ok()?)
}

fn native_thread_id() -> Option<u64> {
    // SAFETY: Linux gettid takes no arguments or pointers and changes no scheduling state.
    let tid = unsafe { libc::syscall(libc::SYS_gettid) };
    u64::try_from(tid).ok().filter(|tid| *tid != 0)
}

impl Record {
    /// Records trigger timing using only an opaque stream identity and local clocks.
    /// No stream, name, buffer or `PipeWire` time metadata is accessed across threads.
    pub(crate) fn trigger(operation: Operation, stream: usize) -> Self {
        Self::empty(operation, stream, false)
    }

    fn empty(operation: Operation, stream: usize, buffer_present: bool) -> Self {
        Self {
            operation,
            stream,
            monotonic_ns: monotonic_ns(),
            native_thread_id: native_thread_id(),
            native_thread_cpu_ns: matches!(
                operation,
                Operation::ProcessEnter | Operation::ProcessExit
            )
            .then(|| clock_ns(libc::CLOCK_THREAD_CPUTIME_ID))
            .flatten(),
            name: [0; 32],
            result: 0,
            buffer_present,
            native_ticks: 0,
            native_report_ns: 0,
            rate_num: 0,
            rate_denom: 0,
            native_time_result: -libc::EINVAL,
            buffered_frames: None,
            queued_buffers: None,
            avail_buffers: None,
            buffer_time_ns: None,
            header: None,
            geometry: None,
            payload: Payload::default(),
        }
    }

    fn include_time(&mut self, time: &sys::pw_time, result: i32) {
        self.native_time_result = result;
        if result == 0 {
            self.native_ticks = time.ticks;
            self.native_report_ns = time.now;
            self.rate_num = time.rate.num;
            self.rate_denom = time.rate.denom;
            // These fields have been present since PipeWire 0.3.50. The sys
            // binding is generated from the private lab's installed headers.
            self.buffered_frames = Some(time.buffered);
            self.queued_buffers = Some(time.queued_buffers);
            self.avail_buffers = Some(time.avail_buffers);
        }
    }

    // SAFETY: stream/name are native-owned and live during the intercepted call.
    // Buffer is null or exclusively dequeued; snapshot it BEFORE queue returns
    // ownership. initialized_pcm requires input delivery or a caller-filled output;
    // a freshly dequeued output may contain uninitialized bytes. No pointers escape.
    pub(crate) unsafe fn snapshot(
        operation: Operation,
        stream: *mut sys::pw_stream,
        buffer: *mut sys::pw_buffer,
        initialized_pcm: bool,
    ) -> Self {
        let mut result = Self::empty(operation, stream as usize, !buffer.is_null());
        if stream.is_null() {
            return result;
        }
        // SAFETY: live stream; bounded read of its immutable, null-terminated name.
        let name: *const c_char = unsafe { sys::pw_stream_get_name(stream) };
        if !name.is_null() {
            for (index, slot) in result.name.iter_mut().enumerate() {
                // SAFETY: stop at NUL; no byte beyond the live C string is read.
                let byte = unsafe { *name.add(index).cast::<u8>() };
                if byte == 0 {
                    break;
                }
                *slot = byte;
            }
        }
        // SAFETY: native pw_time is POD output storage; get_time_n is RT-safe.
        // The exact installed ABI/output size is used while the stream is live.
        let mut time: sys::pw_time = unsafe { std::mem::zeroed() };
        let time_result =
            unsafe { sys::pw_stream_get_time_n(stream, &raw mut time, size_of::<sys::pw_time>()) };
        result.include_time(&time, time_result);
        // SAFETY: dequeue grants this descriptor until queue; capacity/request are
        // initialized metadata even when an output's chunk/PCM is not initialized.
        if let Some(buffer) = unsafe { buffer.as_ref() } {
            result.geometry = Some(unsafe { geometry(buffer, initialized_pcm) });
            result.buffer_time_ns = capture_buffer_time(operation, buffer, initialized_pcm);
        }
        // SAFETY: the intercepted native dequeue owns this descriptor until queue.
        if initialized_pcm
            && let Some(buffer) = unsafe { buffer.as_ref() }
            && let Some(buffer) = unsafe { buffer.buffer.as_ref() }
        {
            // Only a delivered input has initialized native header metadata.
            // An output producer may fill PCM without writing optional metadata.
            if matches!(operation, Operation::Dequeue) {
                result.header = unsafe { header(buffer) };
            }
            result.payload = unsafe { payload(buffer) };
        }
        result
    }
}

fn capture_buffer_time(
    operation: Operation,
    buffer: &sys::pw_buffer,
    initialized_pcm: bool,
) -> Option<u64> {
    // PipeWire >= 1.0.5 initializes this capture-cycle timestamp before process.
    // No output timestamp semantics are documented; do not inspect it there.
    if operation == Operation::Dequeue && initialized_pcm {
        Some(buffer.time)
    } else {
        None
    }
}

// SAFETY: the intercepted call owns the live descriptor. Only initialized_pcm
// permits reading a chunk or its PCM; the returned value retains no pointers.
unsafe fn geometry(buffer: &sys::pw_buffer, initialized_pcm: bool) -> BufferGeometry {
    let mut result = BufferGeometry {
        requested: buffer.requested,
        maxsize: None,
        chunk: None,
    };
    // SAFETY: the native descriptor owns its SPA descriptor through this call.
    let Some(buffer) = (unsafe { buffer.buffer.as_ref() }) else {
        return result;
    };
    if buffer.n_datas != 1 || buffer.datas.is_null() {
        return result;
    }
    // SAFETY: exactly one admitted native data descriptor.
    let data = unsafe { &*buffer.datas };
    result.maxsize = Some(data.maxsize);
    if !initialized_pcm {
        return result;
    }
    // SAFETY: an initialized input or caller-filled output owns this chunk.
    let Some(chunk) = (unsafe { data.chunk.as_ref() }) else {
        return result;
    };
    let bounds_valid = !data.data.is_null()
        && chunk.offset <= data.maxsize
        && chunk.size <= data.maxsize - chunk.offset;
    let edges = if bounds_valid {
        // Sizes above u8::MAX always copy the full eight-byte edge.
        let count = u8::try_from(chunk.size).map_or(8, |size| size.min(8));
        let mut edges = ByteEdges {
            count,
            ..ByteEdges::default()
        };
        // SAFETY: the initialized chunk range was validated against native
        // capacity. These two copies touch at most eight bytes each, unaligned.
        unsafe {
            let first = data.data.cast::<u8>().add(chunk.offset as usize);
            let last = first.add((chunk.size - u32::from(count)) as usize);
            std::ptr::copy_nonoverlapping(first, edges.first.as_mut_ptr(), usize::from(count));
            std::ptr::copy_nonoverlapping(last, edges.last.as_mut_ptr(), usize::from(count));
        }
        Some(edges)
    } else {
        None
    };
    result.chunk = Some(ChunkGeometry {
        offset: chunk.offset,
        size: chunk.size,
        stride: chunk.stride,
        bounds_valid,
        edges,
    });
    result
}

unsafe fn header(buffer: &spa::sys::spa_buffer) -> Option<Header> {
    if buffer.metas.is_null() || buffer.n_metas > 64 {
        return None;
    }
    for index in 0..buffer.n_metas as usize {
        // SAFETY: descriptor owns n_metas entries; index is bounded above.
        let meta = unsafe { &*buffer.metas.add(index) };
        if meta.type_ == spa::sys::SPA_META_Header
            && !meta.data.is_null()
            && meta.size as usize >= size_of::<spa::sys::spa_meta_header>()
        {
            // SAFETY: native meta bounds checked; copy permits unaligned storage
            // and does not retain a reference across handoff.
            let value = unsafe {
                meta.data
                    .cast::<spa::sys::spa_meta_header>()
                    .read_unaligned()
            };
            return Some(Header {
                flags: value.flags,
                pts: value.pts,
                sequence: value.seq,
            });
        }
    }
    None
}

unsafe fn payload(buffer: &spa::sys::spa_buffer) -> Payload {
    if buffer.n_datas != 1 || buffer.datas.is_null() {
        return Payload::default();
    }
    // SAFETY: one native data descriptor; null/malformed chunks are rejected.
    let data = unsafe { &*buffer.datas };
    let Some(chunk) = (unsafe { data.chunk.as_ref() }) else {
        return Payload::default();
    };
    if data.data.is_null()
        || chunk.offset > data.maxsize
        || chunk.size > data.maxsize - chunk.offset
        || chunk.size > 65_536
    {
        return Payload::default();
    }
    // SAFETY: the complete native byte range is bounds-checked and live through
    // this call. Byte access requires no float alignment; no pointer escapes.
    let bytes = unsafe {
        std::slice::from_raw_parts(
            data.data.cast::<u8>().add(chunk.offset as usize),
            chunk.size as usize,
        )
    };
    Payload::measure(bytes, chunk.stride)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_time_metadata_copies_pod_fields_without_allocation() {
        // SAFETY: pw_time consists only of native scalar fields and a POD fraction.
        let mut time: sys::pw_time = unsafe { std::mem::zeroed() };
        time.now = 42_666_667;
        time.ticks = 2048;
        time.rate.num = 1;
        time.rate.denom = 48_000;
        time.buffered = 128;
        time.queued_buffers = 3;
        time.avail_buffers = 2;
        let measured = allocation_counter::measure(|| {
            let mut record = Record::empty(Operation::ProcessEnter, 0, false);
            record.include_time(&time, 0);
            assert_eq!(
                (record.native_report_ns, record.native_ticks),
                (42_666_667, 2048)
            );
            assert_eq!((record.rate_num, record.rate_denom), (1, 48_000));
            assert_eq!(record.buffered_frames, Some(128));
            assert_eq!(
                (record.queued_buffers, record.avail_buffers),
                (Some(3), Some(2))
            );
            let mut failed = Record::empty(Operation::ProcessExit, 0, false);
            failed.include_time(&time, -libc::EIO);
            assert_eq!(failed.native_time_result, -libc::EIO);
            assert_eq!((failed.native_report_ns, failed.native_ticks), (0, 0));
            assert_eq!(failed.buffered_frames, None);
            assert_eq!((failed.queued_buffers, failed.avail_buffers), (None, None));
        });
        assert_eq!(measured.count_total, 0);
    }

    #[test]
    fn capture_buffer_time_is_optional_for_initialized_input_dequeue_only() {
        // SAFETY: the fixture initializes every pw_buffer POD metadata field;
        // there is no SPA descriptor or PCM to inspect.
        let mut buffer: sys::pw_buffer = unsafe { std::mem::zeroed() };
        buffer.time = 42_666_667;
        let measured = allocation_counter::measure(|| {
            assert_eq!(
                capture_buffer_time(Operation::Dequeue, &buffer, true),
                Some(42_666_667)
            );
            assert_eq!(
                capture_buffer_time(Operation::Dequeue, &buffer, false),
                None
            );
            assert_eq!(capture_buffer_time(Operation::Queue, &buffer, true), None);
            assert_eq!(
                capture_buffer_time(Operation::ProcessEnter, &buffer, false),
                None
            );
        });
        assert_eq!(measured.count_total, 0);
    }

    #[test]
    fn process_phase_null_snapshots_preserve_thread_and_optional_metadata() {
        assert_eq!(Operation::process_phase(0), None);
        assert_eq!(Operation::process_phase(3), None);
        assert_eq!(Operation::process_phase(u32::MAX), None);
        for (phase, name) in [(1, "ProcessEnter"), (2, "ProcessExit")] {
            let operation = Operation::process_phase(phase).unwrap();
            let measured = allocation_counter::measure(|| {
                // SAFETY: null stream returns before any native stream call;
                // null buffer admits no descriptor, chunk, metadata or PCM read.
                let record = unsafe {
                    Record::snapshot(operation, std::ptr::null_mut(), std::ptr::null_mut(), false)
                };
                assert!(!record.buffer_present);
                assert!(record.geometry.is_none() && record.header.is_none());
                assert!(record.buffer_time_ns.is_none() && record.buffered_frames.is_none());
                assert!(record.queued_buffers.is_none() && record.avail_buffers.is_none());
                assert!(record.native_thread_id.is_some() && record.monotonic_ns.is_some());
                assert!(record.native_thread_cpu_ns.is_some());
                assert_eq!(record.native_time_result, -libc::EINVAL);
                assert!(!record.payload.valid);
            });
            assert_eq!(measured.count_total, 0);
            let record = Record::empty(operation, 0, false);
            let encoded = serde_json::to_value(record).unwrap();
            assert_eq!(encoded["operation"], name);
            assert!(encoded["native_thread_id"].is_number());
            assert!(encoded["native_thread_cpu_ns"].is_number());
            assert!(encoded["buffer_time_ns"].is_null());
        }
    }

    #[test]
    fn trigger_records_use_only_stream_identity_and_local_clocks_without_allocation() {
        for (operation, name) in [
            (Operation::TriggerEnter, "TriggerEnter"),
            (Operation::TriggerExit, "TriggerExit"),
        ] {
            let measured = allocation_counter::measure(|| {
                // A synthetic identity must never be dereferenced or passed to PipeWire.
                let record = Record::trigger(operation, 1);
                assert_eq!(record.stream, 1);
                assert_eq!(record.operation, operation);
                assert_eq!(record.name, [0; 32]);
                assert!(record.monotonic_ns.is_some() && record.native_thread_id.is_some());
                assert!(record.native_thread_cpu_ns.is_none());
                assert!(!record.buffer_present && !record.payload.valid);
                assert!(record.geometry.is_none() && record.header.is_none());
                assert!(record.buffer_time_ns.is_none() && record.buffered_frames.is_none());
                assert!(record.queued_buffers.is_none() && record.avail_buffers.is_none());
                assert_eq!(record.native_time_result, -libc::EINVAL);
                assert_eq!((record.native_ticks, record.native_report_ns), (0, 0));
                assert_eq!((record.rate_num, record.rate_denom), (0, 0));
            });
            assert_eq!(measured.count_total, 0);
            let mut record = Record::trigger(operation, 0);
            record.result = -libc::EIO;
            let encoded = serde_json::to_value(record).unwrap();
            assert_eq!(encoded["operation"], name);
            assert_eq!(encoded["stream"], 0);
            assert_eq!(encoded["result"], -libc::EIO);
            assert_eq!(encoded["name"], "");
            assert!(encoded["native_thread_cpu_ns"].is_null());
        }
    }

    #[test]
    fn cpu_clock_is_monotonic_and_failed_queries_do_not_invent_zero_time() {
        let before = clock_ns(libc::CLOCK_THREAD_CPUTIME_ID).unwrap();
        let after = clock_ns(libc::CLOCK_THREAD_CPUTIME_ID).unwrap();
        assert!(after >= before);
        assert_eq!(clock_ns(-1), None);
    }

    #[test]
    fn payload_measurement_is_bounded_and_detects_content_changes_without_allocation() {
        let mut bytes = [1; 1024];
        let a = Payload::measure(&bytes, 8);
        assert!(a.valid);
        assert_eq!(a.bytes, 1024);
        bytes[500] = 2;
        assert_ne!(a.hash, Payload::measure(&bytes, 8).hash);
        assert!(!Payload::measure(&bytes[..7], 8).valid);
        assert!(!Payload::measure(&bytes, 4).valid);
        assert!(!Payload::measure(&vec![0; 65_544], 8).valid);
        let measured = allocation_counter::measure(|| {
            std::hint::black_box(Payload::measure(&bytes, 8));
        });
        assert_eq!(measured.count_total, 0);
        assert!(monotonic_ns().is_some());
        // SAFETY: both null inputs exercise rejection before any native access.
        let empty = unsafe {
            Record::snapshot(
                Operation::Dequeue,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                false,
            )
        };
        assert!(!empty.buffer_present && !empty.payload.valid);
        for operation in [Operation::Open, Operation::Queue, Operation::Close] {
            let changed = Record { operation, ..empty };
            assert!(!serde_json::to_string(&changed).unwrap().is_empty());
        }
    }

    #[test]
    fn multichannel_geometry_is_bounded_and_never_reads_uninitialized_output_chunks() {
        let mut bytes = std::array::from_fn::<_, 112, _>(|index| u8::try_from(index).unwrap());
        let mut chunk = spa::sys::spa_chunk {
            offset: 8,
            size: 96,
            stride: 96,
            flags: 0,
        };
        // SAFETY: these native POD descriptors admit zero optional flags/pointers;
        // fixture-owned chunk and PCM live through every geometry/payload call.
        let mut data: spa::sys::spa_data = unsafe { std::mem::zeroed() };
        data.data = bytes.as_mut_ptr().cast();
        data.maxsize = u32::try_from(bytes.len()).unwrap();
        data.chunk = &raw mut chunk;
        // SAFETY: only n_datas/datas are used; optional native metadata is absent.
        let mut spa_buffer: spa::sys::spa_buffer = unsafe { std::mem::zeroed() };
        spa_buffer.n_datas = 1;
        spa_buffer.datas = &raw mut data;
        // SAFETY: only buffer/requested are used; no native stream API is called.
        let mut buffer: sys::pw_buffer = unsafe { std::mem::zeroed() };
        buffer.buffer = &raw mut spa_buffer;
        buffer.requested = 1024;
        let allocations = allocation_counter::measure(|| {
            // SAFETY: fixture retains the complete initialized byte/chunk range.
            let value = unsafe { geometry(&buffer, true) };
            assert_eq!(value.requested, 1024);
            assert_eq!(value.maxsize, Some(112));
            let chunk = value.chunk.unwrap();
            assert_eq!((chunk.offset, chunk.size, chunk.stride), (8, 96, 96));
            assert!(chunk.bounds_valid);
            let edges = chunk.edges.unwrap();
            assert_eq!(edges.count, 8);
            assert_eq!(edges.first, [8, 9, 10, 11, 12, 13, 14, 15]);
            assert_eq!(edges.last, [96, 97, 98, 99, 100, 101, 102, 103]);
            // SAFETY: the same initialized fixture is deliberately not stereo F32.
            assert!(!unsafe { payload(&spa_buffer) }.valid);
        });
        assert_eq!(allocations.count_total, 0);
        chunk.size = 105;
        assert_eq!(chunk.size, 105);
        // SAFETY: descriptor is live, but invalid geometry must reject PCM access.
        let invalid = unsafe { geometry(&buffer, true) }.chunk.unwrap();
        assert!(!invalid.bounds_valid && invalid.edges.is_none());
        chunk.size = 3;
        assert_eq!(chunk.size, 3);
        // SAFETY: the fixture again admits a complete initialized three-byte chunk.
        let short = unsafe { geometry(&buffer, true) }
            .chunk
            .unwrap()
            .edges
            .unwrap();
        assert_eq!(short.count, 3);
        assert_eq!(short.first, [8, 9, 10, 0, 0, 0, 0, 0]);
        assert_eq!(short.first, short.last);
        let mut uninitialized_chunk = MaybeUninit::<spa::sys::spa_chunk>::uninit();
        data.chunk = uninitialized_chunk.as_mut_ptr();
        assert_eq!(data.chunk, uninitialized_chunk.as_mut_ptr());
        // SAFETY: output metadata is live; initialized_pcm=false excludes its chunk.
        let output = unsafe { geometry(&buffer, false) };
        assert_eq!(output.maxsize, Some(112));
        assert!(output.chunk.is_none());
    }
}
