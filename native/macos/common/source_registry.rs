//! Shared-source mapping ownership and single-consumer clock policy.
//! Control publishes immutable slot pointers; callbacks only claim atomics.
//! Old producer mappings stay isolated after replacement and are reclaimed on
//! control after local readers drain, never on an audio callback.
use crate::{
    application_clock::{
        ApplicationClock, ConsumerRoute, VERSION, mr_application_clock_request_rate,
    },
    application_source_mapping::Mapping,
    shared_audio_buffer::{
        mr_shared_audio_buffer_read, mr_shared_audio_buffer_request_epoch,
        mr_shared_audio_buffer_requires_replacement, mr_shared_audio_buffer_stats,
        mr_shared_audio_buffer_try_begin_epoch,
    },
    source_control::SourceRequest,
};
use std::{
    cell::UnsafeCell,
    ffi::c_char,
    io,
    marker::PhantomPinned,
    os::fd::{IntoRawFd, OwnedFd},
    pin::Pin,
    ptr,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
    },
};

use mixrelay_time::{Instant, MonotonicClock};

const MAX_SOURCES: usize = 64;
const MAX_RETAINED: usize = 128;
pub mod content_latency;
mod latency;
mod timestamps;
use latency::{Latency, Observation, transition};
pub use timestamps::{GraphCredit, PublishedCredit, ReadMiss};
const MAX_LEASE: u64 = (1 << 48) - 1;

/// Opaque `SourceClock::epoch`: a lease above its 16-bit shared-buffer epoch.
const fn stream_epoch(lease: u64, epoch: u64) -> u64 {
    (lease << 16) | epoch
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct SourceClock {
    /// Opaque lease/stream identity, not a raw shared-buffer epoch.
    pub epoch: u64,
    pub native_frames: u64,
    pub native_rate: u32,
    pub graph_rate: u32,
    pub running: u32,
    pub synchronized: u32,
}

/// PCM availability and clock metadata observed under one retained source pin.
#[derive(Clone, Copy, Debug)]
pub struct SourcePcmProgress {
    pub clock: SourceClock,
    pub published_frames: u64,
    pub capacity_frames: u32,
}
#[derive(Clone, Copy)]
struct ReadWindow {
    observation: Observation,
    lease: u64,
    format: u64,
    graph_ns: Option<u64>,
}

#[derive(Default)]
struct Timeline {
    policy: Latency,
    previous_read: Option<Instant>,
    previous_frames: u32,
}
impl Timeline {
    fn observe(
        &mut self,
        source: &Source,
        stats: crate::shared_audio_buffer::SharedAudioBufferStats,
        frames: u32,
        timing: (u32, Instant),
    ) -> Observation {
        let clock = source.mapping.clock();
        let (rate, now) = timing;
        // Full scheduling bursts may comprise many graph slices. Accumulate
        // adjacent calls, independently of concurrent producer publications.
        let adjacent = self
            .previous_read
            .filter(|previous| now >= *previous)
            .map(|previous| now.duration_since(previous))
            .is_some_and(|elapsed| {
                elapsed.as_nanos() * u128::from(rate) * 2
                    < u128::from(self.previous_frames) * 1_000_000_000
            });
        self.previous_read = Some(now);
        self.previous_frames = frames;
        Observation {
            epoch: stats.epoch,
            published: stats.published_through_frame,
            frames,
            capacity: source.capacity,
            producer_burst: clock.producer_burst_frames.load(Ordering::Acquire),
            converter_burst: clock.converter_burst_frames.load(Ordering::Acquire),
            drought: clock.publication_drought_frames.load(Ordering::Acquire),
            running: clock.native_running.load(Ordering::Acquire) != 0,
            adjacent,
            copy_missing: 0,
        }
    }

    fn copy_audio(
        &mut self,
        source: &Source,
        samples: &mut [f32],
        policy: Latency,
        window: ReadWindow,
    ) -> u32 {
        let ReadWindow {
            observation,
            lease,
            format,
            graph_ns,
        } = window;
        let frames = observation.frames;
        let age = graph_ns.and_then(|label| {
            content_latency::capture_age(source, observation.epoch, policy.next, frames, label)
        });
        // SAFETY: the caller holds this source's consumer claim and mapping pin;
        // samples is the complete exclusive destination for the validated frame count.
        let available = unsafe {
            mr_shared_audio_buffer_read(
                source.mapping.buffer.as_ptr(),
                policy.next,
                samples.as_mut_ptr(),
                frames,
            )
        };
        let complete =
            source.read_is_current(lease, format, observation.epoch) && available == frames;
        if complete {
            self.policy = policy.consumed(frames);
            source.measurements.age.record(age, u64::from(frames));
        } else if source.read_is_current(lease, format, observation.epoch) {
            self.policy = transition(
                policy,
                Observation {
                    copy_missing: frames - available,
                    ..observation
                },
            )
            .0;
        }
        self.publish(source, observation.epoch, frames, complete);
        if complete {
            available
        } else {
            samples.fill(0.0);
            0
        }
    }

    fn publish(&self, source: &Source, epoch: u64, frames: u32, running: bool) {
        self.publish_telemetry(source.mapping.clock());
        source
            .mapping
            .clock()
            .publish_consumer(crate::application_clock::ConsumerPublication {
                epoch,
                next_frame: self.policy.next,
                frames,
                running,
                target_backlog_frames: self.policy.target,
                phase_ns: self.policy.timing.phase_ns(),
                period_ns: self.policy.timing.period_ns(),
            });
    }

    fn publish_telemetry(&self, clock: &ApplicationClock) {
        let policy = self.policy;
        clock
            .starvation_episodes
            .store(policy.starvation_episodes, Ordering::Release);
        clock
            .starvation_frames
            .store(policy.starvation_frames, Ordering::Release);
        clock
            .rebuffer_frames
            .store(policy.rebuffer_frames, Ordering::Release);
        clock
            .overwrite_skips
            .store(policy.overwrite_skips, Ordering::Release);
        clock
            .consumer_burst_frames
            .store(policy.consumer_burst, Ordering::Release);
        clock
            .insufficient_headroom
            .store(u32::from(policy.insufficient_headroom), Ordering::Release);
    }
}
struct Source {
    id: Vec<u8>,
    token: u64,
    capacity: u32,
    channels: u32,
    mapping: Mapping,
    consumer_busy: AtomicBool,
    timeline: UnsafeCell<Timeline>,
    measurements: content_latency::Measurements,
    // Published slot addresses cannot change while any callback retains a pin.
    _pinned: PhantomPinned,
}
// SAFETY: immutable fields and mapping atomics are shared. Only a successful
// consumer_busy claim permits timeline mutation; failed claims never wait.
unsafe impl Send for Source {}
unsafe impl Sync for Source {}
struct Consumer<'a>(&'a Source);
impl Drop for Consumer<'_> {
    fn drop(&mut self) {
        self.0.consumer_busy.store(false, Ordering::Release);
    }
}
struct Producer<'a>(&'a ApplicationClock);
impl Drop for Producer<'_> {
    fn drop(&mut self) {
        self.0.producer_guard.store(0, Ordering::Release);
    }
}
impl Source {
    /// Complete fallible allocation before changing either producer lease.
    fn grant(&self, lease: u64, previous: Option<&Self>) -> io::Result<Attachment> {
        // Duplicate before any lease/epoch mutation: XPC construction and an
        // immediate subsequent replacement cannot invalidate this owned grant.
        let descriptor = self.mapping.descriptor.try_clone()?;
        let epoch = unsafe { mr_shared_audio_buffer_try_begin_epoch(self.mapping.buffer.as_ptr()) };
        if epoch == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "source stream could not rearm",
            ));
        }
        let clock = self.mapping.clock();
        if let Some(old) = previous {
            let old_clock = old.mapping.clock();
            // The low format-word half carries the requested rate.
            let [r0, r1, r2, r3, ..] = old_clock.format_word.load(Ordering::Acquire).to_le_bytes();
            let rate = u32::from_le_bytes([r0, r1, r2, r3]);
            if rate != 0 {
                unsafe {
                    mr_application_clock_request_rate(self.mapping.clock.as_ptr(), rate);
                }
            }
            clock.clock_master.store(
                old_clock.clock_master.load(Ordering::Acquire),
                Ordering::Release,
            );
            // The old producer claim excludes new additions. A graph reader
            // already pinning the old mapping may drain concurrently; atomic
            // take assigns those replacements to that reader or this mapping,
            // never both. All fallible preparation completed before this move.
            clock
                .source_sample_faults
                .record(old_clock.source_sample_faults.take());
            old_clock.acknowledged_format.store(0, Ordering::Release);
            old_clock.native_running.store(0, Ordering::Release);
            old_clock.producer_lease.store(0, Ordering::Release);
            // SAFETY: attach holds this source's producer claim and its mapping
            // remains pinned. Revoke old PCM as well as the lease before publishing
            // the replacement, including an active (not yet requested) MAX epoch.
            unsafe {
                mr_shared_audio_buffer_request_epoch(old.mapping.buffer.as_ptr());
            }
        }
        clock.acknowledged_format.store(0, Ordering::Release);
        clock.native_running.store(0, Ordering::Release);
        clock.producer_lease.store(lease, Ordering::Release);
        Ok(Attachment {
            descriptor,
            mapped_size: self.mapping.bytes,
            producer_lease: lease,
            source_epoch: epoch,
        })
    }

    fn claim_consumer(&self) -> Option<Consumer<'_>> {
        self.consumer_busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Consumer(self))
    }
    fn claim_producer(&self) -> Option<Producer<'_>> {
        let clock = self.mapping.clock();
        clock
            .producer_guard
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Producer(clock))
    }
    fn terminal(&self) -> bool {
        (unsafe { mr_shared_audio_buffer_requires_replacement(self.mapping.buffer.as_ptr()) })
            || unsafe { mr_shared_audio_buffer_stats(self.mapping.buffer.as_ptr()) }.epoch
                == u64::from(u16::MAX)
            || self.mapping.clock().format_word.load(Ordering::Acquire) >> 32 == u64::from(u32::MAX)
    }

    /// Recheck the nonzero lease/format/epoch observed before copying PCM. The
    /// read pin retains this mapping even when control unpublishes its slot.
    fn read_is_current(&self, lease: u64, format: u64, epoch: u64) -> bool {
        let clock = self.mapping.clock();
        clock.acknowledged_format.load(Ordering::Acquire) == format
            && lease == clock.producer_lease.load(Ordering::Acquire)
            && epoch == unsafe { mr_shared_audio_buffer_stats(self.mapping.buffer.as_ptr()) }.epoch
    }
}
struct Control {
    owners: Vec<Pin<Box<Source>>>,
    next_lease: u64,
}
#[cfg(any(test, feature = "test-observation"))]
type TestStorage = fn(usize) -> io::Result<OwnedFd>;

pub struct SourceRegistry {
    clock: Box<dyn MonotonicClock>,
    timebase: mixrelay_realtime::Timebase,
    #[cfg(any(test, feature = "test-observation"))]
    storage: Option<TestStorage>,
    slots: [AtomicPtr<Source>; MAX_SOURCES],
    readers: AtomicUsize,
    control: Mutex<Control>,
}
struct Read<'a> {
    registry: &'a SourceRegistry,
    source: *const Source,
}
impl Read<'_> {
    fn source(&self) -> Option<&Source> {
        unsafe { self.source.as_ref() }
    }
}
impl Drop for Read<'_> {
    fn drop(&mut self) {
        self.registry.readers.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct Attachment {
    pub descriptor: OwnedFd,
    pub mapped_size: usize,
    pub producer_lease: u64,
    pub source_epoch: u64,
}
impl SourceRegistry {
    /// Publishes how the graph's active plan uses a source to its producer. Returns
    /// false when the identified source is absent.
    pub fn set_consumer_route(&self, token: u64, route: ConsumerRoute) -> bool {
        let read = self.read(token);
        let Some(source) = read.source() else {
            return false;
        };
        // Release publishes the route to the producer's next retention decision.
        source
            .mapping
            .clock()
            .consumer_route
            .store(route.word(), Ordering::Release);
        true
    }
    /// Selects pacing authority without changing PCM, format, or producer ownership.
    pub fn set_clock_master(&self, token: u64, master: bool) -> bool {
        let read = self.read(token);
        let Some(source) = read.source() else {
            return false;
        };
        // Release publishes the pacing choice to the producer before its next conversion.
        source
            .mapping
            .clock()
            .clock_master
            .store(u32::from(master), Ordering::Release);
        true
    }
    /// Creates a registry with explicit scheduling time and host-clock conversion.
    #[must_use]
    pub fn new(clock: Box<dyn MonotonicClock>, timebase: mixrelay_realtime::Timebase) -> Self {
        Self::with_clock(clock, timebase)
    }
    /// Injects test backing storage while retaining the production mapping and IPC layout.
    #[cfg(any(test, feature = "test-observation"))]
    #[must_use]
    pub fn with_clock_and_storage(
        clock: Box<dyn MonotonicClock>,
        timebase: mixrelay_realtime::Timebase,
        storage: fn(usize) -> io::Result<OwnedFd>,
    ) -> Self {
        Self {
            storage: Some(storage),
            ..Self::with_clock(clock, timebase)
        }
    }
    #[cfg_attr(
        not(any(test, feature = "test-observation")),
        expect(
            clippy::unused_self,
            reason = "only test builds inject mapping storage through the registry"
        )
    )]
    fn mapping(&self, capacity: u32, channels: u32) -> io::Result<Mapping> {
        #[cfg(any(test, feature = "test-observation"))]
        if let Some(storage) = self.storage {
            return Mapping::with_storage(capacity, channels, storage);
        }
        Mapping::new(capacity, channels)
    }
    /// Creates a registry with its owned scheduling-clock boundary.
    #[must_use]
    pub fn with_clock(
        clock: Box<dyn MonotonicClock>,
        timebase: mixrelay_realtime::Timebase,
    ) -> Self {
        Self {
            clock,
            timebase,
            #[cfg(any(test, feature = "test-observation"))]
            storage: None,
            slots: std::array::from_fn(|_| AtomicPtr::new(ptr::null_mut())),
            readers: AtomicUsize::new(0),
            control: Mutex::new(Control {
                owners: Vec::new(),
                next_lease: 1,
            }),
        }
    }
    fn control(&self) -> MutexGuard<'_, Control> {
        self.control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// Whether a prepared producer still has a lease and storage for another stream.
    /// Control checks this before handing queues to a pump, including idle epoch exhaustion.
    pub fn producer_can_rearm(&self, token: u64) -> bool {
        let read = self.read(token);
        read.source().is_some_and(|source| {
            !source.terminal()
                && source
                    .mapping
                    .clock()
                    .producer_lease
                    .load(Ordering::Acquire)
                    != 0
        })
    }
    /// Closes only the local grant whose attachment failed; a later lease survives.
    pub(crate) fn cancel_attachment(&self, lease: u64) {
        let mut control = self.control();
        for slot in &self.slots {
            // SAFETY: control owns every slot until collection below, and its mutex
            // excludes grant replacement while this failed local attachment closes.
            if let Some(source) = unsafe { slot.load(Ordering::SeqCst).as_ref() } {
                // AcqRel revokes exactly the failed lease; it cannot close a newer producer.
                let _ = source.mapping.clock().producer_lease.compare_exchange(
                    lease,
                    0,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
        }
        self.collect(&mut control);
    }
    /// Reclaims closed producers on control after their endpoint workers have joined.
    /// Existing graph pins retain mappings until a later collection observes no readers.
    pub fn collect_retired(&self) {
        self.collect(&mut self.control());
    }
    fn collect(&self, control: &mut Control) {
        // Attach and collection share the control mutex. A zero lease means its
        // unique producer closed, not that an active producer is merely paused.
        // Unpublish first; existing readers still pin the retired owner below.
        for slot in &self.slots {
            let source = slot.load(Ordering::SeqCst);
            // SAFETY: all published pointers remain control-owned until this
            // collector observes no readers after removing their slot pointers.
            if unsafe { source.as_ref() }.is_some_and(|source| {
                source
                    .mapping
                    .clock()
                    .producer_lease
                    .load(Ordering::Acquire)
                    == 0
            }) {
                slot.store(ptr::null_mut(), Ordering::SeqCst);
            }
        }
        if self.readers.load(Ordering::SeqCst) == 0 {
            control.owners.retain(|owner| {
                self.slots
                    .iter()
                    .any(|slot| ptr::eq(slot.load(Ordering::SeqCst), owner.as_ref().get_ref()))
            });
        }
    }
    fn read(&self, token: u64) -> Read<'_> {
        self.readers.fetch_add(1, Ordering::SeqCst);
        let source = self
            .slots
            .iter()
            .find_map(|slot| {
                let source = slot.load(Ordering::SeqCst);
                // The reader increment pins every pointer before inspection.
                unsafe { source.as_ref() }
                    .filter(|source| source.token == token)
                    .map(ptr::from_ref)
            })
            .unwrap_or(ptr::null());
        Read {
            registry: self,
            source,
        }
    }
    /// # Errors
    /// Invalid geometry/identity, exhausted leases, busy producers, allocation
    /// failure, or bounded retirement pressure reject without publishing state.
    pub fn attach(&self, request: &SourceRequest) -> io::Result<Attachment> {
        let id = validated_id(request)?;
        let token = token(&id);
        let mut control = self.control();
        self.collect(&mut control);
        if control.next_lease > MAX_LEASE {
            return Err(io::Error::other("source lease space exhausted"));
        }
        let existing = self.slots.iter().enumerate().find_map(|(index, slot)| {
            let pointer = slot.load(Ordering::SeqCst);
            unsafe { pointer.as_ref() }
                .filter(|source| source.token == token)
                .map(|source| (index, source))
        });
        if existing.is_some_and(|(_, source)| {
            source.id != id
                || source.capacity != request.capacity_frames
                || source.channels != request.channels
        }) {
            return Err(io::Error::other("source identity/geometry conflict"));
        }
        let index = existing
            .map(|(index, _)| index)
            .or_else(|| {
                self.slots
                    .iter()
                    .position(|slot| slot.load(Ordering::SeqCst).is_null())
            })
            .ok_or_else(|| io::Error::other("source inventory full"))?;
        let old_producer = match existing {
            Some((_, source)) => Some(source.claim_producer().ok_or_else(|| {
                io::Error::new(io::ErrorKind::WouldBlock, "source producer busy")
            })?),
            None => None,
        };
        let replace = existing.is_none_or(|(_, source)| source.terminal());
        if replace && control.owners.len() >= MAX_RETAINED {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "source readers must drain",
            ));
        }
        let mut candidate = if replace {
            Some(Box::pin(Source {
                id,
                token,
                capacity: request.capacity_frames,
                channels: request.channels,
                mapping: self.mapping(request.capacity_frames, request.channels)?,
                consumer_busy: AtomicBool::new(false),
                timeline: UnsafeCell::new(Timeline::default()),
                measurements: content_latency::Measurements::default(),
                _pinned: PhantomPinned,
            }))
        } else {
            None
        };
        let source = candidate
            .as_deref()
            .or_else(|| existing.map(|(_, source)| source))
            .ok_or_else(|| io::Error::other("source absent"))?;
        let attachment = source.grant(
            control.next_lease,
            existing.filter(|_| replace).map(|(_, old)| old),
        )?;
        control.next_lease += 1;
        if let Some(candidate) = candidate.take() {
            // The pin remains control-owned. Readers access only immutable
            // fields, atomics and the independently claimed timeline cell.
            let pointer = ptr::from_ref(candidate.as_ref().get_ref()).cast_mut();
            control.owners.push(candidate);
            self.slots[index].store(pointer, Ordering::SeqCst);
        }
        drop(old_producer);
        self.collect(&mut control);
        Ok(attachment)
    }
}

fn validated_id(request: &SourceRequest) -> io::Result<Vec<u8>> {
    let id = request
        .source_id
        .iter()
        .position(|byte| *byte == 0)
        .filter(|length| *length != 0)
        .map(|length| {
            request.source_id[..length]
                .iter()
                .map(|byte| byte.to_ne_bytes()[0])
                .collect::<Vec<_>>()
        })
        .ok_or_else(|| io::Error::other("invalid source identity"))?;
    if request.transport_version != VERSION
        || !(1024..=1_048_576).contains(&request.capacity_frames)
        || !request.capacity_frames.is_power_of_two()
        || !(1..=64).contains(&request.channels)
    {
        return Err(io::Error::other("invalid source format"));
    }
    Ok(id)
}

fn token(id: &[u8]) -> u64 {
    let hash = id
        .iter()
        .fold(14_695_981_039_346_656_037_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(1_099_511_628_211)
        });
    if hash == 0 { 1 } else { hash }
}

impl SourceRegistry {
    /// Observes the current producer lease and acknowledged native clock.
    #[must_use]
    pub fn clock(&self, token: u64) -> Option<SourceClock> {
        self.pcm_progress(token).map(|progress| progress.clock)
    }

    /// Observes a source clock and its retained graph-frame extent under one pin.
    #[must_use]
    pub fn pcm_progress(&self, token: u64) -> Option<SourcePcmProgress> {
        let read = self.read(token);
        let source = read.source()?;
        let clock = source.mapping.clock();
        let format = clock.format_word.load(Ordering::Acquire);
        let acknowledged = clock.acknowledged_format.load(Ordering::Acquire);
        let lease = clock.producer_lease.load(Ordering::Acquire);
        // SAFETY: the read pin retains the validated buffer through this snapshot.
        let pcm = unsafe { mr_shared_audio_buffer_stats(source.mapping.buffer.as_ptr()) };
        let epoch = pcm.epoch;
        let frames = clock.native_frames.load(Ordering::Acquire);
        let rate = clock.native_sample_rate.load(Ordering::Acquire);
        let running = clock.native_running.load(Ordering::Acquire);
        let consumer = clock.consumer_position.load(Ordering::Acquire);
        let consumer_running = clock.consumer_running.load(Ordering::Acquire) != 0;
        if format == 0
            || format != acknowledged
            || epoch == 0
            || lease == 0
            || lease > MAX_LEASE
            || format != clock.format_word.load(Ordering::Acquire)
            || acknowledged != clock.acknowledged_format.load(Ordering::Acquire)
            || lease != clock.producer_lease.load(Ordering::Acquire)
            || epoch
                != unsafe { mr_shared_audio_buffer_stats(source.mapping.buffer.as_ptr()) }.epoch
        {
            return None;
        }
        Some(SourcePcmProgress {
            clock: SourceClock {
                epoch: stream_epoch(lease, epoch),
                native_frames: frames,
                native_rate: rate,
                graph_rate: u32::try_from(format & u64::from(u32::MAX)).ok()?,
                running,
                synchronized: u32::from(
                    running != 0 && consumer_running && consumer >> 48 == epoch,
                ),
            },
            published_frames: pcm.published_through_frame,
            capacity_frames: source.capacity,
        })
    }
    /// Reads one complete graph block under the source's single-consumer claim.
    pub fn read_audio(&self, token: u64, samples: &mut [f32], frames: u32, rate: u32) -> u32 {
        self.read_audio_time(
            token,
            samples,
            frames,
            content_latency::ReadTiming {
                rate,
                ..Default::default()
            },
        )
    }

    /// Reads PCM at an optional acquisition cursor and measures its age at the graph label.
    pub fn read_audio_time(
        &self,
        token: u64,
        samples: &mut [f32],
        frames: u32,
        timing: content_latency::ReadTiming,
    ) -> u32 {
        self.read_audio_timed(token, samples, frames, timing)
            .unwrap_or(0)
    }

    /// Reads one complete block like `read_audio_time`, reporting why none was read.
    ///
    /// # Errors
    /// Returns the [`ReadMiss`] reason; `samples` is then silent and the cursor not advanced.
    pub fn read_audio_timed(
        &self,
        token: u64,
        samples: &mut [f32],
        frames: u32,
        timing: content_latency::ReadTiming,
    ) -> Result<u32, ReadMiss> {
        let content_latency::ReadTiming {
            rate,
            first_nanos: requested_time,
            host_ticks,
        } = timing;
        samples.fill(0.0);
        let read = self.read(token);
        let source = read
            .source()
            .filter(|source| {
                frames != 0 && samples.len() == frames as usize * source.channels as usize
            })
            .ok_or(ReadMiss::Unavailable)?;
        let _claim = source.claim_consumer().ok_or(ReadMiss::Unavailable)?;
        let mut attempt = content_latency::Attempt::new(&source.measurements);
        // SAFETY: this nonblocking consumer claim is the only access to the
        // timeline; no control method modifies it, including replacement.
        let timeline = unsafe { &mut *source.timeline.get() };
        let buffer = source.mapping.buffer.as_ptr();
        let clock_pointer = source.mapping.clock.as_ptr();
        let clock = source.mapping.clock();
        let stats = unsafe { mr_shared_audio_buffer_stats(buffer) };
        let format = unsafe { mr_application_clock_request_rate(clock_pointer, rate) };
        let lease = clock.producer_lease.load(Ordering::Acquire);
        let valid = lease != 0
            && format != 0
            && stats.epoch != 0
            && clock.acknowledged_format.load(Ordering::Acquire) == format;
        if !valid {
            timeline.policy.started = false;
            timeline.previous_read = None;
            timeline.policy.next = 0;
            // The reader's clock survives an unavailable source; its next read restarts
            // the measured run, so it is never taken for a sequential reader meanwhile.
            timeline.publish(source, stats.epoch, frames, false);
            return Err(ReadMiss::Unavailable);
        }
        let observation = timeline.observe(source, stats, frames, (rate, self.clock.now()));
        let (mut policy, ready) = transition(timeline.policy, observation);
        if !ready {
            if source.read_is_current(lease, format, stats.epoch) {
                timeline.policy = policy;
            }
            timeline.publish(source, stats.epoch, frames, false);
            return Err(ReadMiss::NotReady);
        }
        if let Err(miss) = timestamps::select(
            source,
            stats,
            timeline.policy,
            &mut policy,
            (rate, frames, requested_time),
        ) {
            if source.read_is_current(lease, format, stats.epoch) {
                // A timestamp miss still observes the scheduling burst. Retain its
                // reset/growth without advancing the unread PCM cursor.
                timeline.policy = policy;
            }
            timeline.publish(source, stats.epoch, frames, false);
            return Err(miss);
        }
        let copied = timeline.copy_audio(
            source,
            samples,
            policy,
            ReadWindow {
                observation,
                lease,
                format,
                graph_ns: host_ticks.map(|host| self.timebase.nanoseconds(host)),
            },
        );
        attempt.completed = copied != 0;
        if copied == 0 {
            return Err(ReadMiss::Incomplete);
        }
        Ok(copied)
    }

    /// Takes normalized-sample faults once for the current source lease.
    pub fn take_source_faults(&self, token: u64) -> u64 {
        let read = self.read(token);
        read.source().map_or(0, |source| {
            source.mapping.clock().source_sample_faults.take()
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mr_engine_source_registry_create() -> *mut SourceRegistry {
    Box::into_raw(Box::new(SourceRegistry::new(
        Box::new(mixrelay_time::StdClock::new()),
        mixrelay_realtime::Timebase::current(),
    )))
}
/// # Safety
/// Registry was created by this module; all callers/readers have stopped.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_source_registry_destroy(registry: *mut SourceRegistry) {
    if !registry.is_null() {
        drop(unsafe { Box::from_raw(registry) });
    }
}
/// # Safety
/// Registry/request and all output pointers are live. A successful descriptor
/// is owned by the caller and must be closed after the attachment is consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_source_registry_attach_clocked(
    registry: *mut SourceRegistry,
    request: *const SourceRequest,
    descriptor: *mut i32,
    bytes: *mut usize,
    lease: *mut u64,
    epoch: *mut u64,
) -> bool {
    let (Some(registry), Some(request)) =
        (unsafe { registry.as_ref() }, unsafe { request.as_ref() })
    else {
        return false;
    };
    if descriptor.is_null() || bytes.is_null() || lease.is_null() || epoch.is_null() {
        return false;
    }
    let Ok(attachment) = registry.attach(request) else {
        return false;
    };
    unsafe {
        descriptor.write(attachment.descriptor.into_raw_fd());
        bytes.write(attachment.mapped_size);
        lease.write(attachment.producer_lease);
        epoch.write(attachment.source_epoch);
    }
    true
}
/// # Safety
/// Source ID is a NUL-terminated string of at most 127 bytes, or a readable
/// 128-byte unterminated array (which is rejected).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_source_id_token(id: *const c_char) -> u64 {
    if id.is_null() {
        return 0;
    }
    let mut bytes = [0_u8; 128];
    for index in 0..128 {
        let byte = unsafe { id.add(index).read() }.to_ne_bytes()[0];
        if byte == 0 {
            return if index == 0 {
                0
            } else {
                token(&bytes[..index])
            };
        }
        bytes[index] = byte;
    }
    0
}
/// # Safety
/// Registry is live and output is writable and disjoint from shared memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_source_registry_clock(
    registry: *mut SourceRegistry,
    token: u64,
    output: *mut SourceClock,
) -> bool {
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        return false;
    };
    if output.is_null() {
        return false;
    }
    let Some(clock) = registry.clock(token) else {
        return false;
    };
    unsafe {
        output.write(clock);
    }
    true
}
/// # Safety
/// Registry remains live throughout this control call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_source_registry_set_clock_master(
    registry: *mut SourceRegistry,
    token: u64,
    master: bool,
) -> bool {
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        return false;
    };
    registry.set_clock_master(token, master)
}
/// # Safety
/// Registry is live; samples has frames*channels writable floats and does not
/// alias the transport or another output buffer during this nonblocking call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_source_registry_read_clocked(
    registry: *mut SourceRegistry,
    token: u64,
    samples: *mut f32,
    frames: u32,
    channels: u32,
    rate: u32,
) -> u32 {
    if samples.is_null() || frames > 1_048_576 || channels == 0 || channels > 64 {
        return 0;
    }
    let output =
        unsafe { std::slice::from_raw_parts_mut(samples, frames as usize * channels as usize) };
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        output.fill(0.0);
        return 0;
    };
    registry.read_audio(token, output, frames, rate)
}

/// Drains normalized source samples once, independently of PCM availability.
/// # Safety
/// Registry is null or retained through this call; the graph owns this source consumer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_source_registry_take_source_faults(
    registry: *const SourceRegistry,
    token: u64,
) -> u64 {
    // SAFETY: the read guard pins the source; only its atomic counter is consumed.
    unsafe { registry.as_ref() }.map_or(0, |registry| registry.take_source_faults(token))
}

#[cfg(test)]
mod tests;
