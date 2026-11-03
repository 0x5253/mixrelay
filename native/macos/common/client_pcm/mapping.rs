//! VM ownership and the single-writer, multiple-reader client queue boundary.
use super::{
    ClientPcmClock, ClientPcmClockOwner, ClientPcmTimeline, Header, MAGIC, VERSION,
    clock::ClockMapping,
};
use crate::endpoint_transport::EndpointTransportConfig;
use crate::shared_audio_buffer::{self as audio, SharedAudioBuffer};
use std::{
    ffi::c_void,
    io,
    ptr::{self, NonNull},
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
};

pub(super) type Object = *mut c_void;
unsafe extern "C" {
    fn mmap(
        address: Object,
        bytes: usize,
        protection: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> Object;
    fn munmap(address: Object, bytes: usize) -> i32;
    fn getpagesize() -> i32;
    fn xpc_shmem_create(address: Object, bytes: usize) -> Object;
    fn xpc_shmem_map(object: Object, address: *mut Object) -> usize;
    fn xpc_release(object: Object);
}

#[derive(Debug)]
struct Mapping {
    address: NonNull<Header>,
    bytes: usize,
}
// SAFETY: immutable geometry and aligned shared atomics are the only shared state.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}
impl Mapping {
    fn header(&self) -> &Header {
        // SAFETY: this owner retains an initialized header at the mapping's base.
        unsafe { self.address.as_ref() }
    }
    fn buffer(&self) -> *mut SharedAudioBuffer {
        // SAFETY: validated initialization/attachment includes the complete trailing ring.
        unsafe { self.address.as_ptr().add(1).cast() }
    }
    fn clock(&self) -> Option<ClientPcmClock> {
        self.header().observe(|| {
            // SAFETY: this retained mapping contains the initialized atomic ring.
            unsafe { audio::mr_shared_audio_buffer_stats(self.buffer()) }.published_through_frame
        })
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: every owner unmaps only its own VM address after its local users drain.
        unsafe { munmap(self.address.as_ptr().cast(), self.bytes) };
    }
}

/// Control allocates and grants; serialized HAL `MixOutput` calls publish PCM.
#[derive(Debug)]
pub struct ClientPcmWriter {
    mapping: Mapping,
    grant: NonNull<c_void>,
    clock: Arc<ClientPcmClockOwner>,
    /// Pages the constructor faulted in, recorded for tests.
    #[cfg(test)]
    prefaulted: usize,
}
// SAFETY: XPC grants are retained control objects; PCM/clock payloads use shared atomics.
unsafe impl Send for ClientPcmWriter {}
unsafe impl Sync for ClientPcmWriter {}
impl ClientPcmWriter {
    pub(super) fn geometry(&self) -> super::EndpointGeometry {
        let header = self.mapping.header();
        super::EndpointGeometry {
            id: header.endpoint,
            render_channels: header.channels,
            capture_channels: header.capture_channels,
        }
    }
    /// Creates a file-free shared VM region before exposing any initialized cells.
    /// # Errors
    /// Rejects invalid identity/geometry and failed VM or XPC allocation.
    #[cfg(any(test, feature = "test-observation"))]
    pub fn new(config: EndpointTransportConfig, client: u32) -> io::Result<Self> {
        Self::with_clock(config, client, Arc::new(ClientPcmClockOwner::new()?))
    }

    /// Creates a client queue belonging to an existing endpoint clock owner.
    /// # Errors
    /// Rejects invalid identity/geometry and propagates VM or XPC allocation failures.
    pub fn with_clock(
        config: EndpointTransportConfig,
        client: u32,
        clock: Arc<ClientPcmClockOwner>,
    ) -> io::Result<Self> {
        let EndpointTransportConfig {
            endpoint_id: endpoint,
            capacity_frames: capacity,
            render_channels: channels,
            capture_channels,
        } = config;
        let ring_bytes = audio::mr_shared_audio_buffer_required_bytes(capacity, channels);
        let bytes = size_of::<Header>() + ring_bytes;
        if endpoint == [0; 16]
            || capture_channels > 64
            || ring_bytes == 0
            || u32::try_from(bytes).is_err()
        {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        // MAP_SHARED|MAP_ANON gives XPC a complete VM allocation, never malloc pages.
        // SAFETY: the kernel creates a new disjoint read/write region; fd is unused.
        let address = unsafe { mmap(ptr::null_mut(), bytes, 3, 0x1001, -1, 0) };
        if address as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        let address = NonNull::new(address.cast::<Header>())
            .ok_or_else(|| io::Error::from(io::ErrorKind::OutOfMemory))?;
        let mapping = Mapping { address, bytes };
        // Create the VM grant before touching pages, as required by xpc_shmem_create.
        // SAFETY: mapping owns all bytes from one MAP_SHARED allocation.
        let grant = NonNull::new(unsafe { xpc_shmem_create(address.as_ptr().cast(), bytes) })
            .ok_or_else(|| io::Error::from(io::ErrorKind::OutOfMemory))?;
        // SAFETY: no reader or callback has access until this constructor returns. The
        // kernel zero-fills this new MAP_ANON region, so the ring needs no clearing.
        unsafe {
            address.as_ptr().write(Header {
                magic: MAGIC,
                version: VERSION,
                bytes: u32::try_from(bytes).unwrap_or(0),
                endpoint,
                client,
                channels,
                capacity,
                capture_channels,
                sequence: AtomicU64::new(0),
                epoch: AtomicU64::new(0),
                anchor: AtomicU64::new(0),
                rate: AtomicU32::new(0),
                running: AtomicU32::new(0),
                first: AtomicU64::new(u64::MAX),
                retired: AtomicU64::new(0),
            });
            audio::initialize_zeroed(mapping.buffer().cast(), ring_bytes, capacity, channels);
        }
        // SAFETY: the constructor still owns every byte of the unpublished mapping.
        let prefaulted = unsafe { prefault(address.as_ptr().cast(), bytes) };
        #[cfg(not(test))]
        let _ = prefaulted;
        Ok(Self {
            mapping,
            grant,
            clock,
            #[cfg(test)]
            prefaulted,
        })
    }

    /// Whether a clock publication can open this queue's next PCM epoch. False once the
    /// queue is retired or its ring has used every epoch; control then replaces it.
    #[must_use]
    pub fn can_rearm(&self) -> bool {
        self.mapping.header().retired.load(Ordering::Acquire) == 0
            // SAFETY: this owner retains the initialized ring for its whole lifetime.
            && unsafe { audio::can_begin_epoch(self.mapping.buffer()) }
    }

    /// Holds this queue's producer claim as an in-flight `MixOutput` callback does.
    #[cfg(any(test, feature = "test-observation"))]
    #[must_use]
    pub fn hold_producer(&self) -> Option<impl Drop + '_> {
        // SAFETY: this owner retains the initialized ring for the claim's lifetime.
        unsafe { audio::hold_producer(self.mapping.buffer()) }
    }

    /// Whether this queue can never open another epoch and must be replaced. Unlike
    /// `can_rearm`, a writer callback in flight does not count.
    #[must_use]
    pub fn exhausted(&self) -> bool {
        self.mapping.header().retired.load(Ordering::Acquire) != 0
            // SAFETY: this owner retains the initialized ring for its whole lifetime.
            || unsafe { audio::epoch_exhausted(self.mapping.buffer()) }
    }

    /// Whether this queue publishes under `owner`'s endpoint clock.
    #[must_use]
    pub fn belongs_to(&self, owner: &ClientPcmClockOwner) -> bool {
        std::ptr::eq(owner, self.clock.as_ref())
    }

    #[cfg(test)]
    pub(super) fn region(&self) -> (*mut c_void, usize) {
        (self.mapping.address.as_ptr().cast(), self.mapping.bytes)
    }
    #[cfg(test)]
    pub(super) fn prefaulted(&self) -> usize {
        self.prefaulted
    }

    pub(super) fn clock_grant(&self) -> Object {
        self.clock.grant()
    }
    pub(super) fn membership(&self) -> u64 {
        self.clock.membership()
    }

    pub(super) fn grant(&self) -> Object {
        self.grant.as_ptr()
    }

    /// Control excludes all writer callbacks during clock publication and rearm.
    #[cfg(any(test, feature = "test-observation"))]
    pub fn set_clock(
        &mut self,
        (epoch, anchor): (u64, u64),
        rate: u32,
        running: bool,
        rearm: bool,
    ) -> bool {
        let owner = Arc::clone(&self.clock);
        let Some(publication) = owner.begin(ClientPcmTimeline {
            epoch,
            anchor,
            rate,
            running,
        }) else {
            return false;
        };
        if !publication.configure(self, rearm) {
            return false;
        }
        publication.commit();
        true
    }

    pub(super) fn configure(
        &mut self,
        owner: &ClientPcmClockOwner,
        timeline: ClientPcmTimeline,
        rearm: bool,
    ) -> bool {
        if !self.belongs_to(owner) {
            return false;
        }
        let ClientPcmTimeline {
            epoch,
            anchor,
            rate,
            running,
        } = timeline;
        let header = self.mapping.header();
        if !crate::client_pcm::valid_rate(rate)
            || epoch == 0
            || header.retired.load(Ordering::Acquire) != 0
        {
            return false;
        }
        // AcqRel opens a control transition before any ring epoch or clock fields change.
        header.sequence.fetch_add(1, Ordering::AcqRel);
        // SAFETY: the HAL control owner has drained its sole producer before rearm.
        let reset = !rearm
            || unsafe { audio::mr_shared_audio_buffer_try_begin_epoch(self.mapping.buffer()) } != 0;
        if rearm {
            header.first.store(u64::MAX, Ordering::Release);
        }
        header.epoch.store(epoch, Ordering::Relaxed);
        header.anchor.store(anchor, Ordering::Relaxed);
        header.rate.store(rate, Ordering::Relaxed);
        header
            .running
            .store(u32::from(running && reset), Ordering::Relaxed);
        // Release commits the ring generation and matching nominal clock together.
        header.sequence.fetch_add(1, Ordering::Release);
        reset
    }

    /// One serialized callback publishes an absolute device interval, without waiting.
    #[must_use]
    pub fn write(&self, frame: u64, samples: &[f32], frames: u32) -> bool {
        let header = self.mapping.header();
        if samples.len() != frames as usize * header.channels as usize {
            return false;
        }
        // SAFETY: retained initialized queue, complete PCM slice, serialized HAL writer.
        let written = unsafe {
            let epoch = audio::mr_shared_audio_buffer_stats(self.mapping.buffer()).epoch;
            audio::mr_shared_audio_buffer_write(
                self.mapping.buffer(),
                epoch,
                frame,
                samples.as_ptr(),
                frames,
            )
        };
        if written && frames != 0 && header.first.load(Ordering::Relaxed) == u64::MAX {
            // The sole producer publishes the first completed frame once per native epoch.
            let _ = header.first.compare_exchange(
                u64::MAX,
                frame,
                Ordering::Release,
                Ordering::Relaxed,
            );
        }
        written
    }
    pub fn read_with_coverage(&self, frame: u64, samples: &mut [f32], coverage: &mut [bool]) {
        // SAFETY: control retains this queue through the callback pin and scratch is exclusive.
        unsafe { audio::read_with_frame_coverage(self.mapping.buffer(), frame, samples, coverage) };
    }
    pub fn read(&self, frame: u64, samples: &mut [f32], frames: u32) -> u32 {
        if samples.len() != frames as usize * self.mapping.header().channels as usize {
            samples.fill(0.0);
            return 0;
        }
        // SAFETY: retained queue and complete exclusive destination were checked above.
        unsafe {
            audio::mr_shared_audio_buffer_read(
                self.mapping.buffer(),
                frame,
                samples.as_mut_ptr(),
                frames,
            )
        }
    }
}
impl Drop for ClientPcmWriter {
    fn drop(&mut self) {
        // Release fences future remote reads before dropping this producer's mapping/grant.
        self.mapping.header().retired.store(1, Ordering::Release);
        // SAFETY: this owner releases its one created XPC reference on control.
        unsafe { xpc_release(self.grant.as_ptr()) };
    }
}

/// Faults every page in on control, before any freeze, so the HAL IO thread never takes
/// a zero-fill fault on its first write or read of a page. Returns the pages touched.
/// # Safety
/// `base` owns `bytes` writable bytes that no other thread accesses during the call.
unsafe fn prefault(base: *mut u8, bytes: usize) -> usize {
    // SAFETY: getpagesize has no preconditions.
    let page = usize::try_from(unsafe { getpagesize() })
        .unwrap_or(4096)
        .max(1);
    let mut pages = 0;
    for offset in (0..bytes).step_by(page) {
        // SAFETY: the offset is inside the owned extent. Rewriting the value already
        // there keeps every initialized cell valid.
        unsafe { ptr::write_volatile(base.add(offset), base.add(offset).read()) };
        pages += 1;
    }
    pages
}

/// A remote reader owns its VM mapping independently of the producer's address.
#[derive(Debug)]
pub struct ClientPcmReader {
    mapping: Mapping,
    clock: ClockMapping,
    membership: u64,
}
impl ClientPcmReader {
    pub(super) unsafe fn attach(grant: Object, clock: Object, membership: u64) -> io::Result<Self> {
        if membership == 0 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let mut address = ptr::null_mut();
        // SAFETY: snapshot decoding verifies the live XPC shared-memory object's type.
        let bytes = unsafe { xpc_shmem_map(grant, &raw mut address) };
        let pointer = NonNull::new(address.cast::<Header>())
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
        let mapping = Mapping {
            address: pointer,
            bytes,
        };
        if bytes < size_of::<Header>() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let header = mapping.header();
        let ring_bytes =
            audio::mr_shared_audio_buffer_required_bytes(header.capacity, header.channels);
        if header.magic != MAGIC
            || header.version != VERSION
            || header.endpoint == [0; 16]
            || header.capture_channels > 64
            || ring_bytes == 0
            || size_of::<Header>() + ring_bytes != header.bytes as usize
            || header.bytes as usize > bytes
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        // SAFETY: immutable geometry and exact required ring extent were checked above.
        let attached =
            unsafe { audio::mr_shared_audio_buffer_attach(mapping.buffer().cast(), ring_bytes) };
        if attached.is_null()
            // SAFETY: the preceding attach validated this initialized ring.
            || unsafe { audio::mr_shared_audio_buffer_channels(attached) } != header.channels
            || unsafe { audio::mr_shared_audio_buffer_capacity(attached) } != header.capacity
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        // SAFETY: snapshot decoding validates both immutable grant types before attachment.
        let clock = unsafe { ClockMapping::attach(clock) }?;
        Ok(Self {
            mapping,
            clock,
            membership,
        })
    }
    #[must_use]
    pub fn endpoint(&self) -> [u8; 16] {
        self.mapping.header().endpoint
    }
    /// Whether the producer has released this queue, independent of membership.
    #[cfg(any(test, feature = "test-observation"))]
    #[must_use]
    pub fn producer_retired(&self) -> bool {
        self.mapping.header().retired.load(Ordering::Acquire) != 0
    }
    #[must_use]
    pub fn client(&self) -> u32 {
        self.mapping.header().client
    }
    #[must_use]
    pub fn channels(&self) -> u32 {
        self.mapping.header().channels
    }
    #[must_use]
    pub fn capture_channels(&self) -> u32 {
        self.mapping.header().capture_channels
    }
    #[must_use]
    pub fn capacity(&self) -> u32 {
        self.mapping.header().capacity
    }
    #[must_use]
    pub fn clock(&self) -> Option<ClientPcmClock> {
        let gate = self.clock.observe()?;
        if gate.membership != self.membership {
            return None;
        }
        let mut clock = self.mapping.clock()?;
        if gate.timeline
            != (ClientPcmTimeline {
                epoch: clock.epoch,
                anchor: clock.anchor,
                rate: clock.rate,
                running: clock.running,
            })
            || self.clock.observe()? != gate
        {
            return None;
        }
        clock.incarnation = gate.incarnation;
        Some(clock)
    }
    /// Copies only the requested epoch. A concurrent stop/rearm/retirement returns silence.
    #[must_use]
    pub fn read(
        &self,
        clock: ClientPcmClock,
        frame: u64,
        output: &mut [f32],
        coverage: &mut [bool],
    ) -> bool {
        output.fill(0.0);
        coverage.fill(false);
        if !clock.running || !same_clock(self.clock(), clock) {
            return false;
        }
        // SAFETY: the reader retains its own VM mapping and owns both bounded scratch slices.
        unsafe { audio::read_with_frame_coverage(self.mapping.buffer(), frame, output, coverage) };
        if !same_clock(self.clock(), clock) {
            output.fill(0.0);
            coverage.fill(false);
            return false;
        }
        true
    }
}
use mixrelay_audio_io::client_pcm::{ClientQueueRead, same_clock};
impl ClientQueueRead for ClientPcmReader {
    fn endpoint(&self) -> [u8; 16] {
        self.endpoint()
    }
    fn channels(&self) -> u32 {
        self.channels()
    }
    fn capture_channels(&self) -> u32 {
        self.capture_channels()
    }
    fn capacity(&self) -> u32 {
        self.capacity()
    }
    fn clock(&self) -> Option<ClientPcmClock> {
        self.clock()
    }
    fn read(
        &self,
        clock: ClientPcmClock,
        first: u64,
        output: &mut [f32],
        coverage: &mut [bool],
    ) -> bool {
        self.read(clock, first, output, coverage)
    }
}
