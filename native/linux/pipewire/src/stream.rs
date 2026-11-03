//! Owning wrappers around the native `PipeWire` callback transport. Destruction is
//! control-thread only: native disconnect drains callbacks before freeing data.
use crate::control_time::{Observation, WaitError, wait_for};
use crate::{Connection, ConnectionHandles};
use mixrelay_audio_io::process::ProcessReport;
use std::{
    cell::Cell,
    ffi::{CString, c_char, c_void},
    marker::PhantomData,
    ptr::NonNull,
    sync::Arc,
};

// The writer admits at most half a ring and playback primes with two quanta.
// Retain two maximum RAW transfers without changing the requested node latency.
const OUTPUT_RETENTION_FRAMES: u32 = StreamFormat::MAX_TRANSFER_FRAMES * 2;

mod owners;
use owners::StreamOwner;
pub use owners::StreamOwners;

use mixrelay_audio_io::capture_read;
#[path = "stream/control.rs"]
mod control;
pub use control::{DescriptionChange, StreamProperties};

#[derive(Default)]
#[repr(C)]
struct NativeCounters {
    overflows: u64,
    starvations: u64,
}

#[cfg(any(test, feature = "test-observation"))]
mod diagnostics;
#[cfg(any(test, feature = "test-observation"))]
pub use diagnostics::{
    NativeTransferSnapshot, RejectedTransfer, TRANSFER_SNAPSHOT_WORDS, TransferSnapshot,
};

unsafe extern "C" {
    fn mr_pw_stream_open_pcm(
        connection: *const ConnectionHandles<'_>,
        name: *const c_char,
        description: *const c_char,
        target: *const c_char,
        mode: u32,
        channels: u32,
        sample_rate: u32,
        block_frames: u32,
        manual_output: bool,
        sample_format: u32,
        ring_frames: u32,
    ) -> *mut c_void;
    fn mr_pw_stream_close(stream: *mut c_void);
    fn mr_pw_stream_set_description(stream: *mut c_void, description: *const c_char) -> i32;
    fn mr_pw_stream_node_id(stream: *mut c_void) -> u32;
    fn mr_pw_stream_activate_capture(stream: *mut c_void) -> i32;
    fn mr_pw_stream_return_error(stream: *mut c_void) -> i32;
    fn mr_pw_stream_ready(stream: *mut c_void) -> i32;
    fn mr_pw_stream_failed(stream: *mut c_void) -> i32;
    fn mr_pw_stream_connected(stream: *mut c_void) -> i32;
    fn mr_pw_stream_input_activity(stream: *mut c_void) -> i32;
    fn mr_pw_stream_clock(stream: *mut c_void) -> u64;
    fn mr_pw_stream_format(stream: *mut c_void) -> u64;
    fn mr_pw_stream_native_format(stream: *mut c_void) -> u64;
    fn mr_pw_stream_supported_formats(stream: *mut c_void) -> u32;
    fn mr_pw_stream_quantum(stream: *mut c_void) -> u64;
    fn mr_pw_stream_discard_input(stream: *mut c_void);
    fn mr_pw_stream_counters(stream: *mut c_void) -> NativeCounters;
    #[cfg(any(test, feature = "test-observation"))]
    fn mr_pw_stream_transfer_snapshot(stream: *mut c_void) -> NativeTransferSnapshot;
    fn mr_pw_stream_read(stream: *mut c_void, samples: *mut f32, frames: u32) -> u32;
    fn mr_pw_stream_capture_failures(stream: *mut c_void) -> u64;
    fn mr_pw_stream_write(stream: *mut c_void, samples: *const f32, frames: u32) -> u32;
    fn mr_pw_stream_trigger_output(stream: *mut c_void) -> i32;
}

/// Decodes the fixed native PCM ABI encoding, including packed three-byte S24.
#[must_use]
pub fn native_sample_format(code: u32) -> Option<mixrelay_audio_io::NativeSampleFormat> {
    use mixrelay_audio_io::NativeSampleFormat;
    match code {
        1 => Some(NativeSampleFormat::Float32),
        2 => Some(NativeSampleFormat::Signed16),
        3 => Some(NativeSampleFormat::Signed24),
        4 => Some(NativeSampleFormat::Signed32),
        _ => None,
    }
}

fn decode_native_format(snapshot: u64) -> Option<mixrelay_audio_io::NativeStreamFormat> {
    let bytes = snapshot.to_le_bytes();
    Some(mixrelay_audio_io::NativeStreamFormat {
        sample_rate: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        channels: u16::from_le_bytes([bytes[0], bytes[1]]),
        sample_format: native_sample_format(u32::from(bytes[2]))?,
    })
}

/// Unique serialized PCM owner; drop on control after its processing callback drains.
pub struct NativeStream {
    // Retired on control before the allocation closes its native capture stream.
    input_binding: Option<crate::GraphInputBinding>,
    allocation: Arc<NativeAllocation>,
    channels: usize,
    counters: NativeCounters,
    reported_return_error: i32,
    capture_read: capture_read::CaptureRead,
    native_output: bool,
    sample_rate: u32,
    // PCM cursors/counter baselines still have exactly one movable Rust owner.
    not_sync: PhantomData<Cell<()>>,
}

struct NativeAllocation {
    pointer: NonNull<c_void>,
    // Drop body drains the stream before this connection releases core/context.
    connection: Connection,
    // Fields drop in declaration order: retain ownership until stream close
    // AND the connection's control thread/context teardown have completed.
    _owner: StreamOwner,
    #[cfg(any(test, feature = "test-observation"))]
    observation_owner: u64,
}

// SAFETY: this private token owns a pinned native allocation. Only NativeStream
// can read/write its Rust-side PCM cursor; it is not Clone or Sync. Weak property
// handles can temporarily retain the allocation but expose only loop-locked
// metadata updates, never PCM/counters. Native callbacks own the opposite cursor.
unsafe impl Send for NativeAllocation {}
// SAFETY: concurrent property access uses the native control-loop lock. PCM
// methods are exclusive NativeStream operations; other observations are atomic.
// The final strong owner closes/drains native callbacks on a control thread.
unsafe impl Sync for NativeAllocation {}

/// Stream purpose also determines whether an existing target is required.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamRole<'a> {
    /// OS playback into a published virtual sink.
    VirtualRender,
    /// Graph PCM exposed as a published virtual source.
    VirtualCapture,
    /// Capture one application's pinned `PipeWire` stream serial.
    ApplicationCapture(&'a str),
    /// Capture a pinned physical source serial.
    DeviceInput(&'a str),
    /// Play into a pinned physical sink serial.
    DeviceOutput(&'a str),
    /// Graph-only contribution into a Driver sink, delivered one native cycle later.
    /// This asynchronous boundary permits reading that same device's mixed capture.
    GraphContribution(&'a str),
    /// Independent test playback publisher without a graph-cycle callback.
    #[cfg(any(test, feature = "test-observation"))]
    AutomaticDeviceOutput(&'a str),
}

impl<'a> StreamRole<'a> {
    const fn native_output(self) -> bool {
        match self {
            Self::VirtualCapture | Self::DeviceOutput(_) | Self::GraphContribution(_) => true,
            #[cfg(any(test, feature = "test-observation"))]
            Self::AutomaticDeviceOutput(_) => true,
            Self::VirtualRender | Self::ApplicationCapture(_) | Self::DeviceInput(_) => false,
        }
    }

    const fn retention_frames(self) -> u32 {
        if self.native_output() {
            OUTPUT_RETENTION_FRAMES
        } else {
            mixrelay_audio_io::retention::RING_FRAMES
        }
    }

    const fn manual_output(self) -> bool {
        matches!(
            self,
            Self::VirtualCapture | Self::DeviceOutput(_) | Self::GraphContribution(_)
        )
    }

    // The numeric mode exists only at the C ABI, not at graph/native callers.
    const fn native(self) -> (u32, Option<&'a str>) {
        match self {
            Self::VirtualRender => (0, None),
            Self::VirtualCapture => (1, None),
            Self::ApplicationCapture(target) => (2, Some(target)),
            Self::DeviceInput(target) => (3, Some(target)),
            Self::DeviceOutput(target) => (4, Some(target)),
            Self::GraphContribution(target) => (5, Some(target)),
            #[cfg(any(test, feature = "test-observation"))]
            Self::AutomaticDeviceOutput(target) => (4, Some(target)),
        }
    }
}

/// Requested RAW PCM rate and processing block bound, independent of any DSP graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamFormat {
    /// RAW stream samples per second.
    pub sample_rate: u32,
    /// Largest serialized read/write block.
    pub block_frames: u32,
}

impl StreamFormat {
    /// Maximum whole RAW PCM transfer, measured in this stream's rate domain.
    pub const MAX_TRANSFER_FRAMES: u32 = 32_768;

    fn is_valid(self) -> bool {
        (8_000..=384_000).contains(&self.sample_rate) && (1..=8192).contains(&self.block_frames)
    }
}

#[derive(Clone, Copy)]
struct StreamSetup {
    channels: u16,
    format: StreamFormat,
    sample_format: u32,
}

impl NativeStream {
    /// Test-only opaque identity of the still-owned allocation, including after
    /// `PipeWire` removes its registry node on target disconnect. Never dereferenced.
    #[cfg(any(test, feature = "test-observation"))]
    #[must_use]
    pub fn allocation_id(&self) -> usize {
        self.allocation.pointer.as_ptr() as usize
    }

    /// Prepare one owned native stream off RT; the native factory copies identity strings.
    /// # Errors
    /// Returns an error for invalid C strings or failed native creation.
    #[expect(
        clippy::too_many_arguments,
        reason = "scope, identity, role and PCM geometry are explicit native preparation inputs"
    )]
    pub fn open_format(
        owners: &StreamOwners,
        name: &str,
        description: &str,
        role: StreamRole<'_>,
        channels: u16,
        format: StreamFormat,
    ) -> Result<Self, String> {
        Self::open(
            owners,
            name,
            description,
            role,
            StreamSetup {
                channels,
                format,
                sample_format: 0,
            },
        )
    }

    /// Opens one exact native encoding tuple, with no alternative negotiation.
    /// # Errors
    /// Rejects invalid geometry, identity or failed native preparation.
    #[expect(
        clippy::too_many_arguments,
        reason = "scope, identity, role and PCM geometry are explicit native preparation inputs"
    )]
    pub fn open_pcm_format(
        owners: &StreamOwners,
        name: &str,
        description: &str,
        role: StreamRole<'_>,
        format: mixrelay_audio_io::NativeStreamFormat,
        block_frames: u32,
    ) -> Result<Self, String> {
        use mixrelay_audio_io::NativeSampleFormat;
        let sample_format = match format.sample_format {
            NativeSampleFormat::Float32 => 1,
            NativeSampleFormat::Signed16 => 2,
            NativeSampleFormat::Signed24 => 3,
            NativeSampleFormat::Signed32 => 4,
        };
        Self::open(
            owners,
            name,
            description,
            role,
            StreamSetup {
                channels: format.channels,
                format: StreamFormat {
                    sample_rate: format.sample_rate,
                    block_frames,
                },
                sample_format,
            },
        )
    }

    fn open(
        owners: &StreamOwners,
        name: &str,
        description: &str,
        role: StreamRole<'_>,
        setup: StreamSetup,
    ) -> Result<Self, String> {
        if !(1..=64).contains(&setup.channels) {
            return Err("invalid PipeWire channel count".into());
        }
        if !setup.format.is_valid() {
            return Err("invalid PipeWire stream format".into());
        }
        let StreamSetup {
            channels,
            format,
            sample_format,
        } = setup;
        let name = CString::new(name).map_err(|_| "invalid PipeWire node name")?;
        let description = CString::new(description).map_err(|_| "invalid PipeWire description")?;
        let (mode, target) = role.native();
        if target.is_some_and(str::is_empty) {
            return Err("invalid PipeWire target".into());
        }
        let target = target
            .map(CString::new)
            .transpose()
            .map_err(|_| "invalid PipeWire target")?;
        let connection = Connection::new(c"mixrelay-stream").map_err(|error| error.to_string())?;
        // SAFETY: input strings remain live until copied. The connection is
        // retained beside the stream; NativeAllocation drains the child first.
        // It is unstarted, so preparation cannot race its control-loop callbacks.
        let pointer = unsafe {
            mr_pw_stream_open_pcm(
                &connection.handles(),
                name.as_ptr(),
                description.as_ptr(),
                target.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                mode,
                u32::from(channels),
                format.sample_rate,
                format.block_frames,
                role.manual_output(),
                sample_format,
                role.retention_frames(),
            )
        };
        let pointer = NonNull::new(pointer).ok_or("could not open native PipeWire stream")?;
        let mut allocation = NativeAllocation {
            pointer,
            connection,
            _owner: owners.acquire(),
            #[cfg(any(test, feature = "test-observation"))]
            observation_owner: owners.observation_identity(),
        };
        // Put the child under its drain guard before starting: a start error
        // closes it before reclaiming the context instead of leaking either.
        allocation
            .connection
            .start()
            .map_err(|error| error.to_string())?;
        if !role.native_output() {
            prepare_capture(&mut allocation, format.sample_rate)?;
        }
        Ok(Self {
            input_binding: None,
            allocation: Arc::new(allocation),
            channels: usize::from(channels),
            counters: NativeCounters::default(),
            reported_return_error: 0,
            capture_read: capture_read::CaptureRead::default(),
            native_output: role.native_output(),
            sample_rate: format.sample_rate,
            not_sync: PhantomData,
        })
    }

    /// Non-owning metadata handle. It cannot outlive/revive this acquisition or
    /// borrow PCM; only an in-flight control operation temporarily pins storage.
    #[must_use]
    pub fn properties(&self) -> StreamProperties {
        StreamProperties::new(&self.allocation)
    }

    /// Prepare this capture owner's adapter-completion dependency off RT.
    /// Retained canonical owners keep their existing link across graph edits.
    /// # Errors
    /// Returns an error if capture negotiation times out, the capture adapter
    /// has no exported registry node, or `GraphInputs::bind` fails.
    pub fn bind_inputs(&mut self, inputs: &crate::GraphInputs) -> Result<(), String> {
        if self.native_output || self.input_binding.is_some() {
            return Ok(());
        }
        let clock = mixrelay_time::StdClock::new();
        wait_for(&clock, || {
            if self.ready() {
                Observation::Ready(())
            } else if self.properties().failed() {
                Observation::Failed
            } else {
                Observation::Pending
            }
        })
        .map_err(|error| match error {
            WaitError::Failed => "capture format failed before dependency setup",
            WaitError::TimedOut => "capture format did not negotiate before dependency setup",
        })?;
        let node_id = self
            .properties()
            .node_id()
            .ok_or("capture adapter is not exported")?;
        let monitor = match self.channels {
            1 => "monitor_MONO",
            2 => "monitor_FL",
            _ => "monitor_AUX0",
        };
        self.input_binding = Some(
            inputs
                .bind(node_id, monitor)
                .map_err(|error| error.to_string())?,
        );
        Ok(())
    }

    /// True only after the exact opened rate/channel geometry is negotiated.
    #[must_use]
    pub fn ready(&self) -> bool {
        // SAFETY: this is an atomic observation of our live owned stream.
        let ready = unsafe { mr_pw_stream_ready(self.allocation.pointer.as_ptr()) != 0 };
        ready
            && self.negotiated_format()
                == (u64::from(self.sample_rate) << 32 | self.channels as u64)
    }

    /// Actual streaming state; paused negotiated streams are not active PCM.
    #[must_use]
    pub fn connected(&self) -> bool {
        // SAFETY: atomic state observation; no native control calls on the DSP thread.
        unsafe { mr_pw_stream_connected(self.allocation.pointer.as_ptr()) != 0 }
    }

    /// Compares actual callback clock identity and rate, not requested rates.
    #[must_use]
    pub fn shares_clock(&self, clock_id: u32, rate: u32) -> bool {
        // SAFETY: atomic value copy; never dereferences native IO memory here.
        let observed = unsafe { mr_pw_stream_clock(self.allocation.pointer.as_ptr()) };
        observed != 0 && observed == (u64::from(clock_id) << 32 | u64::from(rate))
    }

    /// Atomic negotiated rate high32/channel low32 snapshot; zero is unavailable.
    #[must_use]
    pub fn negotiated_format(&self) -> u64 {
        // SAFETY: atomic negotiated format snapshot from our owned stream.
        unsafe { mr_pw_stream_format(self.allocation.pointer.as_ptr()) }
    }

    /// Graph frames high32 and native PCM frames low32, observed independently.
    #[must_use]
    pub fn callback_quantum(&self) -> u64 {
        // SAFETY: atomic value snapshot; graph and PCM units remain distinct.
        unsafe { mr_pw_stream_quantum(self.allocation.pointer.as_ptr()) }
    }

    /// Native streaming/inactive evidence without inferring demand from PCM.
    #[must_use]
    pub fn input_activity(&self) -> mixrelay_audio_io::process::InputActivity {
        use mixrelay_audio_io::process::InputActivity;
        // SAFETY: one atomic native state read; ownership keeps the stream live.
        match unsafe { mr_pw_stream_input_activity(self.allocation.pointer.as_ptr()) } {
            1 => InputActivity::Inactive,
            2 => InputActivity::Active,
            _ => InputActivity::Unknown,
        }
    }

    /// Consumer-only discard; never rewinds the concurrent native producer.
    pub fn discard_input(&mut self) {
        // SAFETY: exclusive engine-side consumer. Native rejects output modes;
        // only the consumer cursor changes, never the concurrent producer's.
        unsafe {
            mr_pw_stream_discard_input(self.allocation.pointer.as_ptr());
        }
    }

    /// Advances this owner's diagnostic baseline without changing PCM cursors.
    #[must_use]
    pub fn take_report(&mut self) -> ProcessReport {
        // SAFETY: native only reads atomic counters from the owned live stream.
        let counters = unsafe { mr_pw_stream_counters(self.allocation.pointer.as_ptr()) };
        // SAFETY: the live acquisition publishes the native error atomically.
        let return_error = unsafe { mr_pw_stream_return_error(self.allocation.pointer.as_ptr()) };
        let report = ProcessReport {
            // The opposite (DSP-facing) side already reports through ExternalIo.
            // Include only native-side failures here to avoid double counting.
            input_starvations: if self.native_output {
                counters
                    .starvations
                    .saturating_sub(self.counters.starvations)
            } else {
                0
            },
            output_overflows: if self.native_output {
                0
            } else {
                counters.overflows.saturating_sub(self.counters.overflows)
            },
            discontinuities: u64::from(return_error < 0 && self.reported_return_error == 0),
            sample_faults: mixrelay_audio_io::process::SampleFaults::default(),
        };
        self.counters = counters;
        self.reported_return_error = return_error;
        report
    }

    /// Reads complete interleaved frames; any unavailable block is silenced.
    pub fn read(&mut self, destination: &mut [f32]) -> u32 {
        read_native_capture(&self.allocation, self.channels, destination)
    }

    /// Reads capture PCM and invalidates this source after newly observed native
    /// ingress loss or a native clock/format/state restart, even when queued
    /// valid PCM fills the request. This observation does not locate missing PCM inside
    /// the queue, discard valid PCM, or advance the independent report baseline.
    /// Non-finite samples are replaced before Rust source mixing and graph state;
    /// external `PipeWire` conversion may already have happened before this boundary.
    pub fn read_capture(
        &mut self,
        destination: &mut [f32],
    ) -> mixrelay_audio_io::process::TransferOutcome {
        let allocation = &self.allocation;
        let channels = self.channels;
        self.capture_read.prepare_read(|| {
            // SAFETY: this exclusive input consumer retains the live owner.
            // Discard advances only its cursor; the native producer is untouched.
            unsafe { mr_pw_stream_discard_input(allocation.pointer.as_ptr()) };
        });
        self.capture_read.read(
            destination,
            channels,
            || {
                // SAFETY: this owner pins the stream. C reads the native input
                // failure counter atomically, without borrowing callback IO.
                unsafe { mr_pw_stream_capture_failures(allocation.pointer.as_ptr()) }
            },
            |samples| read_native_capture(allocation, channels, samples),
        )
    }

    /// Completes one graph block for a canonical capture owner. Any read keeps
    /// its queue intact for shared users; unused inputs discard OFF-period PCM.
    /// The first resumed read also fences PCM queued after this block boundary.
    /// Returns whether this canonical owner was read in the completed block.
    pub fn finish_capture_block(&mut self) -> bool {
        if self.native_output {
            return false;
        }
        let allocation = &self.allocation;
        self.capture_read.finish_block(|| {
            // SAFETY: the graph owns the only input consumer. Native discard is
            // bounded and changes only that consumer's cursor, without control IO.
            unsafe { mr_pw_stream_discard_input(allocation.pointer.as_ptr()) };
        })
    }

    /// Writes one complete interleaved block, retaining unread PCM on overflow.
    pub fn write(&mut self, source: &[f32]) -> u32 {
        let Ok(frames) = u32::try_from(source.len() / self.channels) else {
            return 0;
        };
        // SAFETY: source holds frames * channels readable samples; native copies them.
        unsafe { mr_pw_stream_write(self.allocation.pointer.as_ptr(), source.as_ptr(), frames) }
    }

    /// Releases one output after the complete native cycle, never after each DSP slice.
    /// A late release is retained as a separate native rejection count; queued PCM
    /// remains intact and the next cycle retries without stopping the session.
    pub fn trigger_output(&mut self) -> OutputRelease {
        // SAFETY: this serialized DSP publisher retains the output allocation;
        // PipeWire's RT-safe trigger only releases its scheduling dependency.
        let result = unsafe { mr_pw_stream_trigger_output(self.allocation.pointer.as_ptr()) };
        if result < 0 {
            OutputRelease::Rejected(result)
        } else {
            OutputRelease::Released
        }
    }
}

/// Result of releasing one manual output's native scheduling dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputRelease {
    Released,
    Rejected(i32),
}

fn read_native_capture(
    allocation: &NativeAllocation,
    channels: usize,
    destination: &mut [f32],
) -> u32 {
    let frames = destination.len() / channels;
    destination.fill(0.0);
    let Ok(frames) = u32::try_from(frames) else {
        return 0;
    };
    // SAFETY: callers retain the stream's exclusive PCM consumer. The mutable
    // slice holds at least frames * channels samples and the native copy is bounded.
    unsafe {
        mr_pw_stream_read(
            allocation.pointer.as_ptr(),
            destination.as_mut_ptr(),
            frames,
        )
    }
}

fn prepare_capture(allocation: &mut NativeAllocation, rate: u32) -> Result<(), String> {
    let clock = mixrelay_time::StdClock::new();
    let node_id = wait_for(&clock, || {
        // SAFETY: this unique allocation pins the inactive stream and connection.
        let id = unsafe { mr_pw_stream_node_id(allocation.pointer.as_ptr()) };
        if id == u32::MAX {
            Observation::Pending
        } else {
            Observation::Ready(id)
        }
    })
    .map_err(|_| "capture adapter export timed out")?;
    allocation
        .connection
        .prepare_capture(node_id, rate)
        .map_err(|error| error.to_string())?;
    // SAFETY: successful PortConfig readback precedes activation; this unique
    // control owner retains the native stream through the loop-locked call.
    let result = unsafe { mr_pw_stream_activate_capture(allocation.pointer.as_ptr()) };
    if result < 0 {
        return Err(format!("capture activation failed: {result}"));
    }
    Ok(())
}

impl Drop for NativeAllocation {
    fn drop(&mut self) {
        // SAFETY: final strong owner, retired off DSP. No property transaction or
        // PCM owner remains. Native close disconnects/drains before freeing data.
        unsafe {
            mr_pw_stream_close(self.pointer.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests;
