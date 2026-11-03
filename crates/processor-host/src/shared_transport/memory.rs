//! Test backing with the production protocol, alignment and independent region ownership.
use super::{
    AudioLayout, ControlMailbox, Mapping, ParentAudio, SharedHeader, TransportRegion, WorkerAudio,
    WorkerControl, initialize_private, shared_length,
};
use std::{cell::UnsafeCell, sync::Arc};

#[repr(C, align(64))]
struct AlignedBlock([u8; 64]);
struct HeapRegion(Box<[UnsafeCell<AlignedBlock>]>);
// SAFETY: all region bytes are inside UnsafeCell and are accessed only through the production
// protocol's atomic header, single-writer chunk and independent PCM ownership rules.
unsafe impl Sync for HeapRegion {}
// SAFETY: the boxed aligned blocks keep stable initialized bytes until the last Arc drops.
// No mutable slice or reference spanning peer-owned regions is ever created.
unsafe impl TransportRegion for HeapRegion {
    fn byte_length(&self) -> usize {
        self.0.len() * 64
    }
    fn as_ptr(&self) -> *const u8 {
        self.0.as_ptr().cast()
    }
    fn as_mut_ptr(&self) -> *mut u8 {
        self.0.as_ptr().cast_mut().cast()
    }
}

// Mapping admission requires the exact protocol length, not allocation padding.
struct Region {
    heap: HeapRegion,
    length: usize,
}
// SAFETY: the heap owns at least length bytes, with stable pointers and protocol access.
unsafe impl TransportRegion for Region {
    fn byte_length(&self) -> usize {
        self.length
    }
    fn as_ptr(&self) -> *const u8 {
        self.heap.as_ptr()
    }
    fn as_mut_ptr(&self) -> *mut u8 {
        self.heap.as_mut_ptr()
    }
}

struct NoWake;
impl super::WorkerWake for NoWake {
    fn signal(&self, _: &std::sync::atomic::AtomicU64) {}
}

pub(crate) fn endpoints(
    layout: AudioLayout,
    instance_id: crate::instance_id::InstanceId,
) -> (ParentAudio, ControlMailbox, WorkerAudio, WorkerControl) {
    let length = shared_length(layout).unwrap();
    let heap = HeapRegion(
        (0..length.div_ceil(64))
            .map(|_| UnsafeCell::new(AlignedBlock([0; 64])))
            .collect(),
    );
    let region: Arc<dyn TransportRegion> = Arc::new(Region { heap, length });
    // SAFETY: no peer exists yet; this is the only initialization of this private heap.
    unsafe {
        initialize_private(
            region.as_ref(),
            SharedHeader::new(layout, instance_id).unwrap(),
        )
    }
    .unwrap();
    (
        ParentAudio::from_mapping(Mapping::admit(region.clone(), Arc::new(NoWake)).unwrap())
            .unwrap(),
        ControlMailbox::from_mapping(Mapping::admit(region.clone(), Arc::new(NoWake)).unwrap()),
        WorkerAudio::from_mapping(Mapping::admit(region.clone(), Arc::new(NoWake)).unwrap()),
        WorkerControl::from_mapping(Mapping::admit(region, Arc::new(NoWake)).unwrap()),
    )
}
