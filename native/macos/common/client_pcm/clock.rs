//! One shared endpoint clock gates every client during restart and format changes.
use std::{
    ffi::c_void,
    io,
    ptr::{self, NonNull},
    sync::atomic::{AtomicU64, Ordering},
};

type Object = *mut c_void;
const MAGIC: u64 = 0x4d52_5043_4d43_4c31;
#[repr(C)]
struct State {
    magic: u64,
    incarnation: [u8; 16],
    membership: AtomicU64,
    sequence: AtomicU64,
    epoch: AtomicU64,
    anchor: AtomicU64,
    rate: AtomicU64,
    running: AtomicU64,
    retired: AtomicU64,
}
unsafe extern "C" {
    fn arc4random_buf(address: Object, bytes: usize);
    fn mmap(
        address: Object,
        bytes: usize,
        protection: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> Object;
    fn munmap(address: Object, bytes: usize) -> i32;
    fn xpc_shmem_create(address: Object, bytes: usize) -> Object;
    fn xpc_shmem_map(object: Object, address: *mut Object) -> usize;
    fn xpc_release(object: Object);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientPcmTimeline {
    pub epoch: u64,
    pub anchor: u64,
    pub rate: u32,
    pub running: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct ClockObservation {
    pub sequence: u64,
    pub incarnation: [u8; 16],
    pub membership: u64,
    pub timeline: ClientPcmTimeline,
}

#[derive(Debug)]
pub(super) struct ClockMapping {
    address: NonNull<State>,
    bytes: usize,
}
// SAFETY: geometry is immutable and every shared clock field is atomic.
unsafe impl Send for ClockMapping {}
unsafe impl Sync for ClockMapping {}
impl ClockMapping {
    fn state(&self) -> &State {
        // SAFETY: this owner retains the complete initialized clock mapping.
        unsafe { self.address.as_ref() }
    }
    pub(super) unsafe fn attach(grant: Object) -> io::Result<Self> {
        let mut address = ptr::null_mut();
        // SAFETY: snapshot decoding has validated this retained XPC shmem object's type.
        let bytes = unsafe { xpc_shmem_map(grant, &raw mut address) };
        let address = NonNull::new(address.cast::<State>()).ok_or(io::ErrorKind::InvalidData)?;
        let mapping = Self { address, bytes };
        if bytes < size_of::<State>()
            || mapping.state().magic != MAGIC
            || mapping.state().incarnation == [0; 16]
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(mapping)
    }
    pub(super) fn observe(&self) -> Option<ClockObservation> {
        let state = self.state();
        let sequence = state.sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 || state.retired.load(Ordering::Acquire) != 0 {
            return None;
        }
        let timeline = ClientPcmTimeline {
            epoch: state.epoch.load(Ordering::Relaxed),
            anchor: state.anchor.load(Ordering::Relaxed),
            rate: u32::try_from(state.rate.load(Ordering::Relaxed)).ok()?,
            running: state.running.load(Ordering::Relaxed) != 0,
        };
        // Acquire fences pair the complete clock tuple with its control sequence.
        std::sync::atomic::fence(Ordering::Acquire);
        (state.sequence.load(Ordering::Acquire) == sequence
            && state.retired.load(Ordering::Acquire) == 0)
            .then_some(ClockObservation {
                sequence,
                incarnation: state.incarnation,
                membership: state.membership.load(Ordering::Acquire),
                timeline,
            })
    }
}
impl Drop for ClockMapping {
    fn drop(&mut self) {
        // SAFETY: no local references survive the owning producer/reader mapping.
        unsafe { munmap(self.address.as_ptr().cast(), self.bytes) };
    }
}

/// Control-owned endpoint clock, shared by all of its client PCM grants.
#[derive(Debug)]
pub struct ClientPcmClockOwner {
    mapping: ClockMapping,
    grant: NonNull<c_void>,
}
// SAFETY: the XPC object supports concurrent retain/release and clock publication uses CAS.
unsafe impl Send for ClientPcmClockOwner {}
unsafe impl Sync for ClientPcmClockOwner {}
impl ClientPcmClockOwner {
    /// # Errors
    /// Propagates VM allocation failure and rejects a failed XPC grant allocation.
    pub fn new() -> io::Result<Self> {
        let mut incarnation = [0; 16];
        // SAFETY: control owns all 16 bytes; randomness is never requested on PCM callbacks.
        unsafe { arc4random_buf(incarnation.as_mut_ptr().cast(), incarnation.len()) };
        if incarnation == [0; 16] {
            return Err(io::ErrorKind::Other.into());
        }
        let bytes = size_of::<State>();
        // SAFETY: create a fresh shared read/write anonymous region, without filesystem access.
        let raw = unsafe { mmap(ptr::null_mut(), bytes, 3, 0x1001, -1, 0) };
        if raw as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        let address = NonNull::new(raw.cast::<State>()).ok_or(io::ErrorKind::OutOfMemory)?;
        let mapping = ClockMapping { address, bytes };
        // SAFETY: the grant owns only this complete MAP_SHARED allocation, before initialization.
        let grant = NonNull::new(unsafe { xpc_shmem_create(raw, bytes) })
            .ok_or(io::ErrorKind::OutOfMemory)?;
        // SAFETY: the clock is unpublished and this constructor owns all bytes.
        unsafe {
            address.as_ptr().write(State {
                magic: MAGIC,
                incarnation,
                membership: AtomicU64::new(1),
                sequence: AtomicU64::new(0),
                epoch: AtomicU64::new(0),
                anchor: AtomicU64::new(0),
                rate: AtomicU64::new(0),
                running: AtomicU64::new(0),
                retired: AtomicU64::new(0),
            });
        }
        Ok(Self { mapping, grant })
    }
    pub(super) fn grant(&self) -> Object {
        self.grant.as_ptr()
    }
    pub(super) fn membership(&self) -> u64 {
        self.mapping.state().membership.load(Ordering::Acquire)
    }
    /// Publishes a newer complete membership identity. Readers with older grants
    /// hold their cursors until the matching control snapshot has arrived.
    pub fn publish_membership(&self, revision: u64) {
        self.mapping
            .state()
            .membership
            .fetch_max(revision, Ordering::Release);
    }
    /// Retires the complete endpoint before control releases its client collection.
    pub fn retire(&self) {
        self.mapping.state().retired.store(1, Ordering::Release);
    }
    /// Excludes remote readers while control configures the complete client set.
    #[must_use]
    pub fn begin(&self, timeline: ClientPcmTimeline) -> Option<ClockPublication<'_>> {
        if timeline.epoch == 0
            || !super::valid_rate(timeline.rate)
            || self.mapping.state().retired.load(Ordering::Acquire) != 0
        {
            return None;
        }
        let sequence = self.mapping.state().sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 || sequence > u64::MAX - 2 {
            return None;
        }
        self.mapping
            .state()
            .sequence
            .compare_exchange(sequence, sequence + 1, Ordering::AcqRel, Ordering::Relaxed)
            .ok()?;
        Some(ClockPublication {
            owner: self,
            timeline,
            committed: false,
        })
    }
}
impl Drop for ClientPcmClockOwner {
    fn drop(&mut self) {
        self.mapping.state().retired.store(1, Ordering::Release);
        // SAFETY: this control owner releases its one retained VM grant.
        unsafe { xpc_release(self.grant.as_ptr()) };
    }
}

#[derive(Debug)]
pub struct ClockPublication<'a> {
    owner: &'a ClientPcmClockOwner,
    timeline: ClientPcmTimeline,
    committed: bool,
}
impl ClockPublication<'_> {
    pub fn configure(&self, writer: &mut super::ClientPcmWriter, rearm: bool) -> bool {
        writer.configure(self.owner, self.timeline, rearm)
    }
    /// Ends the transition with the previous timeline still published. Only valid before
    /// any writer was configured in this publication.
    pub fn abandon(mut self) {
        self.committed = true;
    }
    pub fn commit(mut self) {
        let state = self.owner.mapping.state();
        state.epoch.store(self.timeline.epoch, Ordering::Relaxed);
        state.anchor.store(self.timeline.anchor, Ordering::Relaxed);
        state
            .rate
            .store(u64::from(self.timeline.rate), Ordering::Relaxed);
        state
            .running
            .store(u64::from(self.timeline.running), Ordering::Relaxed);
        self.committed = true;
    }
}
impl Drop for ClockPublication<'_> {
    fn drop(&mut self) {
        let state = self.owner.mapping.state();
        if !self.committed {
            state.running.store(0, Ordering::Relaxed);
        }
        // Release ends the whole endpoint transition after every client header/ring is ready.
        state.sequence.fetch_add(1, Ordering::Release);
    }
}
