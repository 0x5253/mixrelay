//! Read-only installed-route measurement. No graph/session/default-device writes.
//! A real hardware-input -> `MixRelay` capture route with audible input is required;
//! absence or silence fails explicitly rather than manufacturing continuity proof.
#![cfg(target_os = "macos")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers outside #[test] functions; a failed setup fails the test"
)]

use mixrelay_audio_graph::{ChannelMapping, GraphDocument, NodeConfig};
use mixrelay_audio_io::process::ProcessingState;
use mixrelay_control_protocol::AppStatus;
use mixrelay_local_socket::{ENGINE_SOCKET, socket_directory};
use reqwest::blocking::Client;
use std::{
    collections::BTreeSet,
    error::Error,
    ffi::c_void,
    io, ptr,
    sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const API: &str = "http://mixrelay-engine/api/v1";
const OBSERVATION: Duration = Duration::from_secs(30);
const MAX_CHANNELS: usize = 64;
#[path = "macos_installed_continuity/recording.rs"]
mod recording;

fn require(ok: bool, message: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(io::Error::other(message.into()).into())
    }
}

fn capture_channels(graph: &GraphDocument) -> Result<(String, u16, BTreeSet<u16>)> {
    let mut reached: BTreeSet<_> = graph
        .nodes
        .iter()
        .filter_map(|node| {
            matches!(node.config, NodeConfig::AudioDeviceInput(_)).then_some(node.id.clone())
        })
        .collect();
    require(
        !reached.is_empty(),
        "existing route has no hardware input; graph was not changed",
    )?;
    for _ in 0..graph.nodes.len() {
        let before = reached.len();
        for edge in &graph.edges {
            if reached.contains(&edge.source.node_id) {
                reached.insert(edge.destination.node_id.clone());
            }
        }
        if before == reached.len() {
            break;
        }
    }
    let mut captures = graph.nodes.iter().filter_map(|node| match &node.config {
        NodeConfig::VirtualEndpoint(config) if reached.contains(&node.id) => {
            config.spec.capture.map(|stream| (node, stream.channels))
        }
        _ => None,
    });
    let (node, channels) = captures.next().ok_or(
        "existing hardware input has no route to MixRelay virtual capture; no topology was changed",
    )?;
    require(
        captures.next().is_none(),
        "multiple virtual capture endpoints are ambiguous",
    )?;
    let mut selected = BTreeSet::new();
    for edge in &graph.edges {
        if edge.destination.node_id != node.id || !reached.contains(&edge.source.node_id) {
            continue;
        }
        match &edge.channel_mapping {
            // Auto mapping guarantees channel zero, but may leave higher capture
            // channels unconnected. Never call intentional unused-channel silence loss.
            ChannelMapping::Auto => {
                selected.insert(0);
            }
            ChannelMapping::Explicit { routes } => {
                selected.extend(routes.iter().map(|route| route.destination_channel));
            }
        }
    }
    require(
        !selected.is_empty() && selected.iter().all(|channel| *channel < channels),
        "route has no valid mapped capture channels",
    )?;
    let NodeConfig::VirtualEndpoint(config) = &node.config else {
        unreachable!()
    };
    let id = mixrelay_macos_transport::endpoint_identity::parse_endpoint_id(config.id.as_str())
        .ok_or("invalid endpoint identity")?;
    Ok((
        mixrelay_macos_transport::endpoint_identity::endpoint_uid(id),
        channels,
        selected,
    ))
}

#[derive(Default)]
struct ChannelEvidence {
    pcm: Box<[AtomicU32]>,
    samples: AtomicU64,
    nonzero: AtomicU64,
    invalid: AtomicU64,
    zero_tail: AtomicU64,
    longest_zero: AtomicU64,
}
impl ChannelEvidence {
    fn observe(&self, samples: impl Iterator<Item = f32>) {
        let base = usize::try_from(self.samples.load(Ordering::Relaxed)).unwrap_or(usize::MAX);
        let mut count = 0;
        let mut nonzero = 0;
        let mut invalid = 0;
        let mut tail = self.zero_tail.load(Ordering::Relaxed);
        let mut longest = tail;
        for sample in samples {
            if let Some(destination) = self.pcm.get(base.saturating_add(count)) {
                destination.store(sample.to_bits(), Ordering::Relaxed);
            }
            count += 1;
            if !sample.is_finite() {
                invalid += 1;
            }
            if sample == 0.0 {
                tail += 1;
                longest = longest.max(tail);
            } else {
                nonzero += 1;
                tail = 0;
            }
        }
        self.samples.fetch_add(count as u64, Ordering::Relaxed);
        self.nonzero.fetch_add(nonzero, Ordering::Relaxed);
        self.invalid.fetch_add(invalid, Ordering::Relaxed);
        self.zero_tail.store(tail, Ordering::Relaxed);
        self.longest_zero.fetch_max(longest, Ordering::Relaxed);
    }
}

struct CaptureEvidence {
    first_host_time: AtomicU64,
    enabled: AtomicBool,
    selected: BTreeSet<u16>,
    channels: [ChannelEvidence; MAX_CHANNELS],
    callbacks: AtomicU64,
    frames: AtomicU64,
    malformed: AtomicU64,
    timestamp_gaps: AtomicU64,
    expected_sample_time: AtomicU64,
}
impl CaptureEvidence {
    fn new(selected: BTreeSet<u16>) -> Self {
        let record = std::env::var_os("MIXRELAY_CAPTURE_OUTPUT").is_some();
        let channels = std::array::from_fn(|index| ChannelEvidence {
            pcm: (0..if record
                && selected.contains(&u16::try_from(index).expect("64-channel bound"))
            {
                192_000 * 31
            } else {
                0
            })
                .map(|_| AtomicU32::new(0))
                .collect(),
            ..ChannelEvidence::default()
        });
        Self {
            first_host_time: AtomicU64::new(0),
            enabled: AtomicBool::new(false),
            selected,
            channels,
            callbacks: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            malformed: AtomicU64::new(0),
            timestamp_gaps: AtomicU64::new(0),
            expected_sample_time: AtomicU64::new(f64::NAN.to_bits()),
        }
    }
    fn clock(&self, sample_time: f64, frames: u32) {
        let previous = f64::from_bits(self.expected_sample_time.swap(
            (sample_time + f64::from(frames)).to_bits(),
            Ordering::Relaxed,
        ));
        if !sample_time.is_finite()
            || (previous.is_finite() && (sample_time - previous).abs() > 0.5)
        {
            self.timestamp_gaps.fetch_add(1, Ordering::Relaxed);
        }
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        self.frames.fetch_add(u64::from(frames), Ordering::Relaxed);
    }
    fn validate(&self, rate: u32, buffer: u32, elapsed: Duration) -> Result<()> {
        let frames = self.frames.load(Ordering::Relaxed);
        let expected = u128::from(rate) * elapsed.as_nanos() / 1_000_000_000;
        println!(
            "independent HAL capture: callbacks={} frames={frames} expected_frames={expected} malformed={} timestamp_gaps={}",
            self.callbacks.load(Ordering::Relaxed),
            self.malformed.load(Ordering::Relaxed),
            self.timestamp_gaps.load(Ordering::Relaxed)
        );
        require(
            u128::from(frames) * 100 >= expected * 95,
            "native capture delivered less than 95% of wall-clock frames",
        )?;
        require(
            self.malformed.load(Ordering::Relaxed) == 0
                && self.timestamp_gaps.load(Ordering::Relaxed) == 0,
            "native PCM geometry or sample timestamp continuity failed",
        )?;
        for &channel in &self.selected {
            let evidence = &self.channels[usize::from(channel)];
            let samples = evidence.samples.load(Ordering::Relaxed);
            let nonzero = evidence.nonzero.load(Ordering::Relaxed);
            let longest = evidence.longest_zero.load(Ordering::Relaxed);
            println!(
                "PCM channel={} samples={samples} nonzero={nonzero} longest_exact_zero_frames={longest} invalid={}",
                channel + 1,
                evidence.invalid.load(Ordering::Relaxed)
            );
            require(
                samples == frames && evidence.invalid.load(Ordering::Relaxed) == 0,
                "captured channel had missing or non-finite PCM",
            )?;
            require(
                samples > 0 && nonzero * 100 >= samples * 98,
                "insufficient continuous signal: silence or dropped PCM cannot establish route continuity; supply a sustained real input",
            )?;
            require(
                longest <= u64::from(buffer.max(rate / 500)),
                "PCM contained an exact-zero run longer than one graph buffer or 2 ms",
            )?;
        }
        Ok(())
    }
}

// Minimal direct CoreAudio ABI. This reader does not call MixRelay's native
// bridge, SPSC, meters, or counters, and never reconfigures device formats.
#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}
#[repr(C)]
struct AudioBuffer {
    channels: u32,
    bytes: u32,
    data: *mut c_void,
}
#[repr(C)]
struct BufferList {
    count: u32,
    buffers: [AudioBuffer; 1],
}
#[repr(C)]
struct Timestamp {
    sample_time: f64,
    host_time: u64,
    rate_scalar: f64,
    word_clock: u64,
    smpte: [u8; 24],
    flags: u32,
    reserved: u32,
}
#[repr(C)]
#[derive(Default)]
struct StreamFormat {
    rate: f64,
    id: u32,
    flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels: u32,
    bits: u32,
    reserved: u32,
}
#[repr(C)]
struct StreamUsage {
    proc_id: *mut c_void,
    count: u32,
    enabled: [u32; MAX_CHANNELS],
}
type Callback = unsafe extern "C" fn(
    u32,
    *const Timestamp,
    *const BufferList,
    *const Timestamp,
    *mut BufferList,
    *const Timestamp,
    *mut c_void,
) -> i32;
#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectSetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> i32;
    fn AudioDeviceCreateIOProcID(
        device: u32,
        callback: Callback,
        data: *mut c_void,
        result: *mut *mut c_void,
    ) -> i32;
    fn AudioDeviceStart(device: u32, proc_id: *mut c_void) -> i32;
    fn AudioDeviceStop(device: u32, proc_id: *mut c_void) -> i32;
    fn AudioDeviceDestroyIOProcID(device: u32, proc_id: *mut c_void) -> i32;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        text: *const std::ffi::c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFRelease(object: *const c_void);
}
fn address(selector: [u8; 4], scope: [u8; 4]) -> Address {
    Address {
        selector: u32::from_be_bytes(selector),
        scope: u32::from_be_bytes(scope),
        element: 0,
    }
}
fn os_status(status: i32, label: &str) -> Result<()> {
    require(status == 0, format!("{label}: OSStatus {status}"))
}

unsafe extern "C" fn receive(
    _: u32,
    _: *const Timestamp,
    input: *const BufferList,
    time: *const Timestamp,
    _: *mut BufferList,
    _: *const Timestamp,
    context: *mut c_void,
) -> i32 {
    // SAFETY: NativeCapture keeps this pinned Box alive until unregister succeeds.
    let state = unsafe { &*context.cast::<CaptureEvidence>() };
    if !state.enabled.load(Ordering::Acquire) {
        return 0;
    }
    if input.is_null() || time.is_null() {
        state.malformed.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    let (list, time) = unsafe { (&*input, &*time) };
    let _ = state.first_host_time.compare_exchange(
        0,
        time.host_time,
        Ordering::Relaxed,
        Ordering::Relaxed,
    );
    if list.count == 0 || list.count > 64 || time.flags & 1 == 0 {
        state.malformed.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    let mut offset = 0_u32;
    let mut cycle_frames = None;
    for index in 0..list.count as usize {
        // CoreAudio supplies exactly count buffers in this flexible-array ABI.
        let buffer = unsafe { &*ptr::addr_of!(list.buffers).cast::<AudioBuffer>().add(index) };
        if buffer.channels == 0
            || buffer.channels > 64
            || offset + buffer.channels > 64
            || buffer.bytes % (4 * buffer.channels) != 0
            || buffer.data.is_null()
        {
            state.malformed.fetch_add(1, Ordering::Relaxed);
            return 0;
        }
        let frames = buffer.bytes / (4 * buffer.channels);
        if frames == 0 || frames > 8192 || cycle_frames.is_some_and(|previous| previous != frames) {
            state.malformed.fetch_add(1, Ordering::Relaxed);
            return 0;
        }
        cycle_frames = Some(frames);
        // Format was verified as packed native-endian Float32 before registration.
        let samples = unsafe {
            std::slice::from_raw_parts(buffer.data.cast::<f32>(), (buffer.bytes / 4) as usize)
        };
        for &selected in &state.selected {
            let selected = u32::from(selected);
            if (offset..offset + buffer.channels).contains(&selected) {
                state.channels[selected as usize].observe(
                    samples[(selected - offset) as usize..]
                        .iter()
                        .step_by(buffer.channels as usize)
                        .copied(),
                );
            }
        }
        offset += buffer.channels;
    }
    state.clock(time.sample_time, cycle_frames.unwrap_or(0));
    0
}

struct NativeCapture {
    device: u32,
    proc_id: *mut c_void,
    evidence: Box<CaptureEvidence>,
}
impl NativeCapture {
    fn open(uid: &str, rate: u32, channels: u16, selected: BTreeSet<u16>) -> Result<Self> {
        let device = Self::resolve(uid)?;
        Self::validate_format(device, rate, channels)?;
        let mut capture = Self {
            device,
            proc_id: ptr::null_mut(),
            evidence: Box::new(CaptureEvidence::new(selected)),
        };
        os_status(
            unsafe {
                AudioDeviceCreateIOProcID(
                    device,
                    receive,
                    ptr::from_mut(capture.evidence.as_mut()).cast(),
                    &raw mut capture.proc_id,
                )
            },
            "register independent capture",
        )?;
        require(
            !capture.proc_id.is_null(),
            "CoreAudio returned no capture registration",
        )?;
        // Disable this reader's output streams only. It must not become a silent
        // render producer or affect the user's virtual-source audio.
        let output_count = Self::streams(device, b"outp")?.len();
        if output_count > 0 {
            let usage = StreamUsage {
                proc_id: capture.proc_id,
                count: u32::try_from(output_count)?,
                enabled: [0; MAX_CHANNELS],
            };
            let size =
                u32::try_from(std::mem::offset_of!(StreamUsage, enabled) + output_count * 4)?;
            os_status(
                unsafe {
                    AudioObjectSetPropertyData(
                        device,
                        &address(*b"suse", *b"outp"),
                        0,
                        ptr::null(),
                        size,
                        ptr::from_ref(&usage).cast(),
                    )
                },
                "disable this capture client's outputs",
            )?;
        }
        os_status(
            unsafe { AudioDeviceStart(device, capture.proc_id) },
            "start independent HAL capture",
        )?;
        Ok(capture)
    }
    fn resolve(uid: &str) -> Result<u32> {
        let name = std::ffi::CString::new(uid)?;
        let uid = unsafe { CFStringCreateWithCString(ptr::null(), name.as_ptr(), 0x0800_0100) };
        require(!uid.is_null(), "cannot create owned HAL UID")?;
        let mut device = 0_u32;
        let mut size = 4;
        let status = unsafe {
            AudioObjectGetPropertyData(
                1,
                &address(*b"uidd", *b"glob"),
                u32::try_from(size_of_val(&uid))?,
                ptr::from_ref(&uid).cast(),
                &raw mut size,
                ptr::from_mut(&mut device).cast(),
            )
        };
        unsafe {
            CFRelease(uid);
        }
        os_status(status, "resolve MixRelay HAL")?;
        require(size == 4 && device != 0, "installed own HAL is unavailable")?;
        Ok(device)
    }
    fn validate_format(device: u32, rate: u32, channels: u16) -> Result<()> {
        let input_streams = Self::streams(device, b"inpt")?;
        let mut total = 0;
        for stream in input_streams {
            let mut format = StreamFormat::default();
            let mut size = u32::try_from(size_of::<StreamFormat>())?;
            os_status(
                unsafe {
                    AudioObjectGetPropertyData(
                        stream,
                        &address(*b"sfmt", *b"glob"),
                        0,
                        ptr::null(),
                        &raw mut size,
                        ptr::from_mut(&mut format).cast(),
                    )
                },
                "read HAL input format",
            )?;
            require(
                size as usize == size_of::<StreamFormat>()
                    && format.rate.to_bits() == f64::from(rate).to_bits()
                    && format.id == u32::from_be_bytes(*b"lpcm")
                    && format.flags & 1 != 0
                    && format.flags & 2 == 0
                    && format.bits == 32
                    && format.bytes_per_frame == 4 * format.channels
                    && format.channels > 0
                    && format.channels <= 64,
                "independent reader requires native packed Float32 at the unchanged session rate",
            )?;
            total += format.channels;
        }
        require(
            total == u32::from(channels),
            "installed capture channel count does not match graph",
        )?;
        Ok(())
    }
    fn streams(device: u32, scope: &[u8; 4]) -> Result<Vec<u32>> {
        let mut streams = [0_u32; MAX_CHANNELS];
        let mut size = u32::try_from(size_of_val(&streams))?;
        os_status(
            unsafe {
                AudioObjectGetPropertyData(
                    device,
                    &address(*b"stm#", *scope),
                    0,
                    ptr::null(),
                    &raw mut size,
                    streams.as_mut_ptr().cast(),
                )
            },
            "read HAL streams",
        )?;
        require(
            size as usize <= size_of_val(&streams) && size % 4 == 0,
            "invalid HAL stream list extent",
        )?;
        Ok(streams[..size as usize / 4].to_vec())
    }
    fn observed_rate(&self) -> Result<f64> {
        let mut rate = 0.0_f64;
        let mut size = 8;
        os_status(
            unsafe {
                AudioObjectGetPropertyData(
                    self.device,
                    // kAudioDevicePropertyNominalSampleRate (AudioHardwareBase.h).
                    &address(*b"nsrt", *b"glob"),
                    0,
                    ptr::null(),
                    &raw mut size,
                    ptr::from_mut(&mut rate).cast(),
                )
            },
            "read observed HAL rate",
        )?;
        require(
            size == 8 && rate.is_finite() && rate > 0.0,
            "invalid observed HAL rate",
        )?;
        Ok(rate)
    }
    fn close(&mut self) -> Result<()> {
        if self.proc_id.is_null() {
            return Ok(());
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            unsafe {
                AudioDeviceStop(self.device, self.proc_id);
            }
            let status = unsafe { AudioDeviceDestroyIOProcID(self.device, self.proc_id) };
            if status == 0 {
                self.proc_id = ptr::null_mut();
                return Ok(());
            }
            require(
                Instant::now() < deadline,
                format!("CoreAudio refused reader teardown: {status}"),
            )?;
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for NativeCapture {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            // This is a dedicated test process: a failed unregister must fail it,
            // never free callback storage still borrowed by the OS or leak it.
            eprintln!("fatal owned capture teardown: {error}");
            std::process::exit(70);
        }
    }
}

fn selected_graph_url() -> Result<reqwest::Url> {
    let mut graph_url = reqwest::Url::parse(&format!("{API}/graph"))?;
    if let Ok(id) = std::env::var("MIXRELAY_TEST_GRAPH_ID") {
        graph_url = reqwest::Url::parse(&format!("{API}/graphs/"))?;
        graph_url
            .path_segments_mut()
            .map_err(|()| "invalid graph URL")?
            .pop_if_empty()
            .push(&id);
    }
    Ok(graph_url)
}

#[test]
fn installed_mixed_route_pcm_continuity_without_changing_graph_or_format() -> Result<()> {
    require(
        std::env::var("MIXRELAY_TEST_INSTALLED_ENGINE").as_deref() == Ok("1"),
        "explicit installed read-only measurement authorization required: MIXRELAY_TEST_INSTALLED_ENGINE=1; no I/O started",
    )?;
    let client = Client::builder()
        .unix_socket(socket_directory()?.join(ENGINE_SOCKET))
        .timeout(Duration::from_secs(3))
        .build()?;
    let graph_url = selected_graph_url()?;
    let status = || -> Result<AppStatus> {
        Ok(client
            .get(format!("{API}/status"))
            .send()?
            .error_for_status()?
            .json()?)
    };
    let graph = || -> Result<GraphDocument> {
        Ok(client
            .get(graph_url.clone())
            .send()?
            .error_for_status()?
            .json()?)
    };
    let original_graph = graph()?;
    let initial = status()?;
    let format = initial
        .audio_session
        .as_ref()
        .ok_or("missing installed session")?
        .format;
    let (uid, channels, selected) = capture_channels(&original_graph)?;
    println!(
        "read-only mixed route revision={} format={format:?} capture_channels={channels} selected={selected:?} hardware_output_present={}",
        original_graph.revision,
        original_graph
            .nodes
            .iter()
            .any(|node| matches!(node.config, NodeConfig::AudioDeviceOutput(_)))
    );
    let mut capture = NativeCapture::open(&uid, format.sample_rate, channels, selected)?;
    let native_rate_before = capture.observed_rate()?;
    std::thread::sleep(Duration::from_millis(500));
    // Capture startup and shutdown belong to the acceptance window too. PCM
    // analysis still excludes the initial bounded hardware-reader priming period.
    let before = initial;
    require(
        graph()? == original_graph,
        "graph/revision changed during reader startup",
    )?;
    capture.evidence.enabled.store(true, Ordering::Release);
    let started = Instant::now();
    std::thread::sleep(OBSERVATION);
    capture.evidence.enabled.store(false, Ordering::Release);
    let elapsed = started.elapsed();
    capture.close()?;
    recording::save(&capture.evidence, format.sample_rate)?;
    let after = status()?;
    let after_graph = graph()?;
    let native_rate_after = capture.observed_rate()?;
    require(
        before
            .audio_session
            .as_ref()
            .ok_or("missing before session")?
            .format
            == format,
        "format changed during reader startup",
    )?;
    verify_observations(
        Observation {
            graph: original_graph,
            status: before,
            native_rate: native_rate_before,
        },
        Observation {
            graph: after_graph,
            status: after,
            native_rate: native_rate_after,
        },
        &capture.evidence,
        elapsed,
    )
}

struct Observation {
    graph: GraphDocument,
    status: AppStatus,
    native_rate: f64,
}
fn verify_observations(
    before: Observation,
    after: Observation,
    evidence: &CaptureEvidence,
    elapsed: Duration,
) -> Result<()> {
    let Observation {
        graph: original_graph,
        status: before,
        native_rate: native_rate_before,
    } = before;
    let Observation {
        graph: after_graph,
        status: after,
        native_rate: native_rate_after,
    } = after;
    let format = before
        .audio_session
        .as_ref()
        .ok_or("missing before session")?
        .format;
    let before_session = before
        .audio_session
        .as_ref()
        .ok_or("missing before session")?;
    let after_session = after
        .audio_session
        .as_ref()
        .ok_or("missing after session")?;
    let underruns = after
        .underruns
        .checked_sub(before.underruns)
        .ok_or("engine underrun counter reset during measurement")?;
    let overruns = after
        .overruns
        .checked_sub(before.overruns)
        .ok_or("engine overrun counter reset during measurement")?;
    let discontinuities = after_session
        .discontinuities
        .checked_sub(before_session.discontinuities)
        .ok_or("engine discontinuity counter reset during measurement")?;
    println!(
        "mixed continuity duration_ms={} before_revision={} after_revision={} before_format={:?} after_format={:?} underruns={underruns} overruns={overruns} discontinuities={discontinuities}",
        elapsed.as_millis(),
        original_graph.revision,
        after_graph.revision,
        before_session.format,
        after_session.format
    );
    // Emit provenance alongside optional, explicitly requested PCM artifacts.
    println!(
        "continuity_evidence {}",
        serde_json::to_string(&serde_json::json!({
            "graphBefore": original_graph, "graphAfter": after_graph,
            "statusBefore": before, "statusAfter": after,
            "nativeRateBefore": native_rate_before, "nativeRateAfter": native_rate_after,
            "durationMillis": elapsed.as_millis(),
            "pcm": evidence.selected.iter().map(|channel| {
                let evidence = &evidence.channels[usize::from(*channel)];
                serde_json::json!({"channel": channel + 1, "samples": evidence.samples.load(Ordering::Relaxed), "nonzero": evidence.nonzero.load(Ordering::Relaxed), "invalid": evidence.invalid.load(Ordering::Relaxed), "longestZeroRun": evidence.longest_zero.load(Ordering::Relaxed)})
            }).collect::<Vec<_>>()
        }))?
    );
    let pcm = evidence.validate(format.sample_rate, format.block_frames, elapsed);
    require(
        after_graph == original_graph,
        "graph topology or revision changed during read-only measurement",
    )?;
    require(
        before_session.format == format && after_session.format == format,
        "active format changed during read-only measurement",
    )?;
    require(
        native_rate_before.to_bits() == f64::from(format.sample_rate).to_bits()
            && native_rate_after.to_bits() == native_rate_before.to_bits(),
        "native HAL rate changed during measurement",
    )?;
    require(
        after_session.processing_state == ProcessingState::Running && after_session.issue.is_none(),
        format!("installed engine not healthy: {after_session:?}"),
    )?;
    require(
        underruns == 0 && overruns == 0 && discontinuities == 0,
        "installed engine reported route starvation/overflow/clock discontinuity",
    )?;
    pcm
}

#[test]
fn pcm_evidence_detects_injected_silence_nonfinite_samples_and_timestamp_jumps() {
    assert_eq!(size_of::<Timestamp>(), 64);
    assert_eq!(std::mem::offset_of!(Timestamp, flags), 56);
    assert_eq!(std::mem::offset_of!(BufferList, buffers), 8);
    let evidence = CaptureEvidence::new(BTreeSet::from([0]));
    let signal: Vec<f32> = (0..4800)
        .map(|index| if index % 2 == 0 { 0.125 } else { -0.125 })
        .collect();
    evidence.channels[0].observe(signal.iter().copied());
    evidence.clock(0.0, 4800);
    assert!(
        evidence
            .validate(48_000, 32, Duration::from_millis(100))
            .is_ok()
    );
    evidence.channels[0].observe(std::iter::repeat_n(0.0, 200));
    evidence.clock(4800.0, 200);
    assert!(
        evidence
            .validate(48_000, 32, Duration::from_millis(100))
            .is_err()
    );
    evidence.channels[0].observe([f32::NAN].into_iter());
    assert_eq!(evidence.channels[0].invalid.load(Ordering::Relaxed), 1);
    evidence.clock(9000.0, 32);
    assert_eq!(evidence.timestamp_gaps.load(Ordering::Relaxed), 1);
}
