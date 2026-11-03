//! Host slider changes reach the processor; processor output reaches its controller.
//!
//! The worker serializes control and processing. All COM objects and slot storage
//! are allocated at load; successive edits before one block coalesce to the latest
//! value at offset zero. Output points are allocated before activation and kept
//! separate from input edits. No queue allocation or lock occurs in `process`.

use std::{
    ptr,
    sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering},
};
use truce_rack_core::error::{Error, Result};
use vst3::Steinberg::Vst::{
    IAudioProcessor, IAudioProcessorTrait, IEditController, IEditControllerTrait, IParamValueQueue,
    IParamValueQueueTrait, IParameterChanges, IParameterChangesTrait,
    ParameterInfo as Vst3ParameterInfo, ProcessData,
};
use vst3::Steinberg::{kInvalidArgument, kResultOk};
use vst3::{Class, ComPtr, ComWrapper};

// Matches the containing host's parameter mailbox bound. Reject an unrepresentable
// catalog explicitly rather than silently discarding edits or allocating unboundedly.
const MAX_PARAMETERS: usize = 4096;
const INACTIVE: usize = usize::MAX;

struct ValueQueue {
    id: u32,
    value: AtomicU64,
    pending: AtomicBool,
}

impl Class for ValueQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ValueQueue {
    unsafe fn getParameterId(&self) -> u32 {
        self.id
    }
    unsafe fn getPointCount(&self) -> i32 {
        i32::from(self.pending.load(Ordering::Acquire))
    }
    unsafe fn getPoint(&self, index: i32, offset: *mut i32, value: *mut f64) -> i32 {
        if index != 0
            || !self.pending.load(Ordering::Acquire)
            || offset.is_null()
            || value.is_null()
        {
            return kInvalidArgument;
        }
        // SAFETY: the caller supplies writable outputs; null pointers were rejected.
        unsafe {
            *offset = 0;
            *value = f64::from_bits(self.value.load(Ordering::Acquire));
        }
        kResultOk
    }
    unsafe fn addPoint(&self, _: i32, _: f64, _: *mut i32) -> i32 {
        // The processor receives a read-only input queue, not an output queue.
        kInvalidArgument
    }
}

struct Slot {
    queue: ComWrapper<ValueQueue>,
    pointer: ComPtr<IParamValueQueue>,
}

struct Changes {
    slots: Vec<Slot>,
    active: Vec<AtomicUsize>,
    count: AtomicUsize,
}

impl Class for Changes {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for Changes {
    unsafe fn getParameterCount(&self) -> i32 {
        i32::try_from(self.count.load(Ordering::Acquire)).expect("bounded parameter slots")
    }
    unsafe fn getParameterData(&self, index: i32) -> *mut IParamValueQueue {
        usize::try_from(index)
            .ok()
            .filter(|index| *index < self.count.load(Ordering::Acquire))
            .map(|index| self.active[index].load(Ordering::Acquire))
            .map_or(ptr::null_mut(), |index| self.slots[index].pointer.as_ptr())
    }
    unsafe fn addParameterData(&self, _: *const u32, _: *mut i32) -> *mut IParamValueQueue {
        ptr::null_mut()
    }
}

struct OutputPoint {
    offset: AtomicI32,
    value: AtomicU64,
}

struct OutputQueue {
    id: u32,
    points: Vec<OutputPoint>,
    count: AtomicUsize,
    block_frames: AtomicI32,
}

impl Class for OutputQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for OutputQueue {
    unsafe fn getParameterId(&self) -> u32 {
        self.id
    }

    unsafe fn getPointCount(&self) -> i32 {
        i32::try_from(self.count.load(Ordering::Acquire)).expect("bounded output points")
    }

    unsafe fn getPoint(&self, index: i32, offset: *mut i32, value: *mut f64) -> i32 {
        let Some(point) = usize::try_from(index)
            .ok()
            .filter(|index| *index < self.count.load(Ordering::Acquire))
            .and_then(|index| self.points.get(index))
        else {
            return kInvalidArgument;
        };
        if offset.is_null() || value.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the COM caller supplies writable offset/value outputs for this
        // call; null was rejected. The count's release store publishes the point.
        unsafe {
            *offset = point.offset.load(Ordering::Acquire);
            *value = f64::from_bits(point.value.load(Ordering::Acquire));
        }
        kResultOk
    }

    unsafe fn addPoint(&self, offset: i32, value: f64, index: *mut i32) -> i32 {
        if index.is_null()
            || offset < 0
            || offset >= self.block_frames.load(Ordering::Acquire)
            || !value.is_finite()
            || !(0.0..=1.0).contains(&value)
        {
            return kInvalidArgument;
        }
        let count = self.count.load(Ordering::Relaxed);
        let position = self.points[..count]
            .partition_point(|point| point.offset.load(Ordering::Relaxed) < offset);
        if position < count && self.points[position].offset.load(Ordering::Relaxed) == offset {
            self.points[position]
                .value
                .store(value.to_bits(), Ordering::Release);
        } else {
            if count == self.points.len() {
                return kInvalidArgument;
            }
            for current in (position..count).rev() {
                let previous = &self.points[current];
                let next = &self.points[current + 1];
                next.offset
                    .store(previous.offset.load(Ordering::Relaxed), Ordering::Relaxed);
                next.value
                    .store(previous.value.load(Ordering::Relaxed), Ordering::Relaxed);
            }
            self.points[position]
                .offset
                .store(offset, Ordering::Relaxed);
            self.points[position]
                .value
                .store(value.to_bits(), Ordering::Relaxed);
            self.count.store(count + 1, Ordering::Release);
        }
        // SAFETY: the COM caller supplies a writable index output; null was rejected.
        unsafe { *index = i32::try_from(position).expect("point index fits advertised block") };
        kResultOk
    }
}

struct OutputSlot {
    queue: ComWrapper<OutputQueue>,
    pointer: ComPtr<IParamValueQueue>,
    active_index: AtomicUsize,
}

struct OutputChanges {
    slots: Vec<OutputSlot>,
    by_id: Vec<(u32, usize)>,
    active: Vec<AtomicUsize>,
    count: AtomicUsize,
    block_frames: AtomicI32,
}

impl Class for OutputChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for OutputChanges {
    unsafe fn getParameterCount(&self) -> i32 {
        i32::try_from(self.count.load(Ordering::Acquire)).expect("bounded output parameters")
    }

    unsafe fn getParameterData(&self, index: i32) -> *mut IParamValueQueue {
        usize::try_from(index)
            .ok()
            .filter(|index| *index < self.count.load(Ordering::Acquire))
            .map(|index| self.active[index].load(Ordering::Acquire))
            .map_or(ptr::null_mut(), |index| self.slots[index].pointer.as_ptr())
    }

    unsafe fn addParameterData(&self, id: *const u32, index: *mut i32) -> *mut IParamValueQueue {
        if id.is_null() || index.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: the COM caller supplies a readable ParamID for this call;
        // null was rejected before dereferencing it.
        let id = unsafe { *id };
        let Ok(position) = self.by_id.binary_search_by_key(&id, |(id, _)| *id) else {
            return ptr::null_mut();
        };
        let slot_index = self.by_id[position].1;
        let slot = &self.slots[slot_index];
        let mut active_index = slot.active_index.load(Ordering::Acquire);
        if active_index == INACTIVE {
            active_index = self.count.load(Ordering::Relaxed);
            if active_index >= self.active.len() {
                return ptr::null_mut();
            }
            slot.queue
                .block_frames
                .store(self.block_frames.load(Ordering::Acquire), Ordering::Release);
            self.active[active_index].store(slot_index, Ordering::Release);
            slot.active_index.store(active_index, Ordering::Release);
            self.count.store(active_index + 1, Ordering::Release);
        }
        // SAFETY: the COM caller supplies a writable index output; null was rejected.
        unsafe { *index = i32::try_from(active_index).expect("bounded output parameters") };
        slot.pointer.as_ptr()
    }
}

pub(super) struct OutputParameters {
    changes: ComWrapper<OutputChanges>,
    pointer: ComPtr<IParameterChanges>,
    maximum_block: usize,
    latest: Vec<Option<f64>>,
    pending: Vec<usize>,
}

impl OutputParameters {
    fn new(metadata: &[Vst3ParameterInfo], maximum_block: usize) -> Result<Self> {
        if maximum_block == 0
            || i32::try_from(maximum_block).is_err()
            || metadata
                .len()
                .checked_mul(maximum_block)
                .and_then(|points| points.checked_mul(std::mem::size_of::<OutputPoint>()))
                .is_none()
        {
            return Err(Error::Other(
                "VST3 output parameter storage size is invalid".into(),
            ));
        }
        let mut slots = Vec::with_capacity(metadata.len());
        let mut by_id = Vec::with_capacity(metadata.len());
        for (index, info) in metadata.iter().enumerate() {
            let mut points = Vec::new();
            points.try_reserve_exact(maximum_block).map_err(|_| {
                Error::Other("VST3 output parameter storage allocation failed".into())
            })?;
            points.resize_with(maximum_block, || OutputPoint {
                offset: AtomicI32::new(0),
                value: AtomicU64::new(0),
            });
            let queue = ComWrapper::new(OutputQueue {
                id: info.id,
                points,
                count: AtomicUsize::new(0),
                block_frames: AtomicI32::new(0),
            });
            let pointer = queue
                .to_com_ptr::<IParamValueQueue>()
                .expect("OutputQueue interface");
            slots.push(OutputSlot {
                queue,
                pointer,
                active_index: AtomicUsize::new(INACTIVE),
            });
            by_id.push((info.id, index));
        }
        by_id.sort_unstable_by_key(|(id, _)| *id);
        let changes = ComWrapper::new(OutputChanges {
            slots,
            by_id,
            active: (0..metadata.len()).map(|_| AtomicUsize::new(0)).collect(),
            count: AtomicUsize::new(0),
            block_frames: AtomicI32::new(0),
        });
        let pointer = changes
            .to_com_ptr::<IParameterChanges>()
            .expect("OutputChanges interface");
        Ok(Self {
            changes,
            pointer,
            maximum_block,
            latest: vec![None; metadata.len()],
            pending: Vec::with_capacity(metadata.len()),
        })
    }

    fn begin_block(&self, frames: i32) {
        let count = self.changes.count.load(Ordering::Acquire);
        for active in &self.changes.active[..count] {
            let slot = &self.changes.slots[active.load(Ordering::Acquire)];
            slot.queue.count.store(0, Ordering::Release);
            slot.active_index.store(INACTIVE, Ordering::Release);
        }
        self.changes.count.store(0, Ordering::Release);
        self.changes.block_frames.store(frames, Ordering::Release);
    }

    fn capture_block(&mut self) {
        let count = self.changes.count.load(Ordering::Acquire);
        for active in &self.changes.active[..count] {
            let index = active.load(Ordering::Acquire);
            let slot = &self.changes.slots[index];
            let point_count = slot.queue.count.load(Ordering::Acquire);
            if point_count == 0 {
                continue;
            }
            let value = f64::from_bits(
                slot.queue.points[point_count - 1]
                    .value
                    .load(Ordering::Acquire),
            );
            // An index stays in `pending` until the whole service pass succeeds.
            // Each slot enters once, so the load-time capacity bounds this RT push.
            if self.latest[index].is_none() {
                self.pending.push(index);
            }
            self.latest[index] = Some(value);
        }
    }

    fn service(&mut self, mut apply: impl FnMut(u32, f64) -> bool) -> Result<()> {
        for &index in &self.pending {
            let Some(value) = self.latest[index] else {
                continue;
            };
            let id = self.changes.slots[index].queue.id;
            if !apply(id, value) {
                return Err(Error::Other(
                    "IEditController::setParamNormalized rejected processor output".into(),
                ));
            }
        }
        for &index in &self.pending {
            self.latest[index] = None;
        }
        self.pending.clear();
        Ok(())
    }

    fn discard(&mut self) {
        for &index in &self.pending {
            self.latest[index] = None;
        }
        self.pending.clear();
        self.begin_block(0);
    }
}

pub(super) struct InputParameters {
    changes: ComWrapper<Changes>,
    pointer: ComPtr<IParameterChanges>,
    metadata: Vec<Vst3ParameterInfo>,
    output: OutputParameters,
}

impl InputParameters {
    pub(super) fn from_controller(
        controller: &ComPtr<IEditController>,
        count: usize,
    ) -> Result<Self> {
        if count > MAX_PARAMETERS {
            return Err(Error::Other(
                "VST3 parameter count exceeds host capacity".into(),
            ));
        }
        let mut slots: Vec<Slot> = Vec::with_capacity(count);
        let mut metadata = Vec::with_capacity(count);
        for index in 0..count {
            let mut info = crate::metadata::empty_parameter_info();
            let index = i32::try_from(index).expect("bounded parameter count");
            if unsafe { controller.getParameterInfo(index, &raw mut info) } != kResultOk {
                return Err(Error::Other(
                    "VST3 parameter catalog could not be read".into(),
                ));
            }
            if slots.iter().any(|slot| slot.queue.id == info.id) {
                return Err(Error::Other(
                    "VST3 parameter catalog contains duplicate IDs".into(),
                ));
            }
            let queue = ComWrapper::new(ValueQueue {
                id: info.id,
                value: AtomicU64::new(0.0_f64.to_bits()),
                pending: AtomicBool::new(false),
            });
            let pointer = queue
                .to_com_ptr::<IParamValueQueue>()
                .expect("ValueQueue interface");
            slots.push(Slot { queue, pointer });
            metadata.push(info);
        }
        let changes = ComWrapper::new(Changes {
            slots,
            active: (0..count).map(|_| AtomicUsize::new(0)).collect(),
            count: AtomicUsize::new(0),
        });
        let pointer = changes
            .to_com_ptr::<IParameterChanges>()
            .expect("Changes interface");
        let output = OutputParameters::new(&metadata, 1)?;
        Ok(Self {
            changes,
            pointer,
            metadata,
            output,
        })
    }

    pub(super) fn set(
        &mut self,
        controller: &ComPtr<IEditController>,
        index: usize,
        value: f64,
    ) -> Result<()> {
        if !value.is_finite() {
            return Err(Error::Other("VST3 parameter value must be finite".into()));
        }
        let value = value.clamp(0.0, 1.0);
        let slot = self
            .changes
            .slots
            .get(index)
            .ok_or(Error::InvalidParameter(index))?;
        if unsafe { controller.setParamNormalized(slot.queue.id, value) } != kResultOk {
            return Err(Error::Other(
                "IEditController::setParamNormalized failed".into(),
            ));
        }
        self.queue_index(index, value)
    }

    /// Queue an edit already accepted by a native editor.
    /// This deliberately does not call `setParamNormalized` recursively.
    pub(super) fn queue_id_value(&mut self, id: u32, value: f64) -> Result<()> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(Error::Other(
                "VST3 normalized parameter value is invalid".into(),
            ));
        }
        let index = self
            .changes
            .slots
            .iter()
            .position(|slot| slot.queue.id == id)
            .ok_or_else(|| Error::Other("VST3 callback used an unknown parameter ID".into()))?;
        self.queue_index(index, value)
    }

    fn queue_index(&mut self, index: usize, value: f64) -> Result<()> {
        let slot = self
            .changes
            .slots
            .get(index)
            .ok_or(Error::InvalidParameter(index))?;
        slot.queue.value.store(value.to_bits(), Ordering::Release);
        if !slot.queue.pending.swap(true, Ordering::AcqRel) {
            // Control and DSP are serialized by the owning worker. Atomics also
            // keep retained COM interfaces safe to read from foreign threads; no
            // unsafe Send/Sync claim or interior borrowed references are needed.
            let count = self.changes.count.load(Ordering::Acquire);
            self.changes.active[count].store(index, Ordering::Release);
            self.changes.count.store(count + 1, Ordering::Release);
        }
        Ok(())
    }

    pub(super) fn has_pending_changes(&self) -> bool {
        self.changes.count.load(Ordering::Acquire) != 0
    }

    pub(super) fn metadata(&self, index: usize) -> Option<&Vst3ParameterInfo> {
        self.metadata.get(index)
    }

    /// Apply processor-originated values on the instance's creator thread.
    pub(super) fn service_output(&mut self, controller: &ComPtr<IEditController>) -> Result<()> {
        self.output
            .service(|id, value| unsafe { controller.setParamNormalized(id, value) } == kResultOk)
    }

    /// Invalidate output notifications from the state preceding a restore.
    pub(super) fn discard_output(&mut self) {
        self.output.discard();
    }

    /// Allocate output points before native activation without replacing a live queue.
    pub(super) fn output_replacement(
        &self,
        maximum_block: usize,
    ) -> Result<Option<OutputParameters>> {
        if maximum_block == 0 || i32::try_from(maximum_block).is_err() {
            return Err(Error::Other("invalid VST3 maximum block size".into()));
        }
        if maximum_block <= self.output.maximum_block {
            return Ok(None);
        }
        OutputParameters::new(&self.metadata, maximum_block).map(Some)
    }

    /// Commit a prepared queue only after native activation has succeeded.
    pub(super) fn install_output(&mut self, replacement: Option<OutputParameters>) {
        if let Some(output) = replacement {
            self.output = output;
        }
    }

    /// Refresh titles and flags without allowing an unannounced ID remap.
    pub(super) fn refresh_metadata(&mut self, controller: &ComPtr<IEditController>) -> Result<()> {
        let count = unsafe { controller.getParameterCount() };
        if usize::try_from(count).ok() != Some(self.metadata.len()) {
            return Err(Error::Other(
                "VST3 parameter count changed without supported remapping".into(),
            ));
        }
        for (index, cached) in self.metadata.iter_mut().enumerate() {
            let mut updated = crate::metadata::empty_parameter_info();
            let index = i32::try_from(index).expect("parameter capacity is bounded");
            if unsafe { controller.getParameterInfo(index, &raw mut updated) } != kResultOk {
                return Err(Error::Other(
                    "VST3 parameter metadata refresh failed".into(),
                ));
            }
            if updated.id != cached.id {
                return Err(Error::Other(
                    "VST3 parameter ID remapping is unsupported".into(),
                ));
            }
            *cached = updated;
        }
        Ok(())
    }

    pub(super) fn process(
        &mut self,
        processor: &ComPtr<IAudioProcessor>,
        data: &mut ProcessData,
    ) -> i32 {
        let Ok(frames) = usize::try_from(data.numSamples) else {
            return kInvalidArgument;
        };
        if frames > self.output.maximum_block {
            return kInvalidArgument;
        }
        self.output.begin_block(data.numSamples);
        data.inputParameterChanges = self.pointer.as_ptr();
        data.outputParameterChanges = self.output.pointer.as_ptr();
        let status = unsafe { processor.process(ptr::from_mut(data)) };
        // Do not leave borrowed COM pointers in caller-owned scratch storage.
        data.inputParameterChanges = ptr::null_mut();
        data.outputParameterChanges = ptr::null_mut();
        if status == kResultOk {
            let count = self.changes.count.load(Ordering::Acquire);
            for active in &self.changes.active[..count] {
                let index = active.load(Ordering::Acquire);
                self.changes.slots[index]
                    .queue
                    .pending
                    .store(false, Ordering::Release);
            }
            self.changes.count.store(0, Ordering::Release);
            self.output.capture_block();
        }
        status
    }
}

#[cfg(test)]
mod output_tests {
    use super::*;
    use vst3::ComRef;

    fn metadata(id: u32) -> Vst3ParameterInfo {
        let mut info = crate::metadata::empty_parameter_info();
        info.id = id;
        info
    }

    #[test]
    fn output_queues_are_separate_and_clear_only_block_points() {
        let mut output = OutputParameters::new(&[metadata(11), metadata(7)], 8).unwrap();
        output.begin_block(8);
        let unknown = 42_u32;
        let mut index = -1;
        assert!(
            unsafe {
                output
                    .pointer
                    .addParameterData(&raw const unknown, &raw mut index)
            }
            .is_null()
        );
        assert!(unsafe { output.pointer.addParameterData(ptr::null(), &raw mut index) }.is_null());

        let id = 7_u32;
        let raw = unsafe {
            output
                .pointer
                .addParameterData(&raw const id, &raw mut index)
        };
        assert!(!raw.is_null());
        assert_eq!(index, 0);
        assert_eq!(unsafe { output.pointer.getParameterCount() }, 1);
        assert_eq!(unsafe { output.pointer.getParameterData(0) }, raw);
        let repeated = unsafe {
            output
                .pointer
                .addParameterData(&raw const id, &raw mut index)
        };
        assert_eq!(repeated, raw);
        assert_eq!(index, 0);
        assert_eq!(unsafe { output.pointer.getParameterCount() }, 1);

        let queue = unsafe { ComRef::<IParamValueQueue>::from_raw(raw) }
            .expect("known output queue pointer");
        assert_eq!(unsafe { queue.getParameterId() }, id);
        assert_eq!(
            unsafe { queue.addPoint(7, 0.875, &raw mut index) },
            kResultOk
        );
        output.capture_block();
        assert_eq!(output.latest.as_slice(), &[None, Some(0.875)]);
        assert_eq!(output.pending.as_slice(), &[1]);

        output.begin_block(8);
        assert_eq!(unsafe { output.pointer.getParameterCount() }, 0);
        assert_eq!(unsafe { queue.getPointCount() }, 0);
        assert_eq!(output.latest.as_slice(), &[None, Some(0.875)]);
        assert_eq!(output.pending.as_slice(), &[1]);
    }

    #[test]
    fn output_points_cover_the_advertised_block_with_sorted_upserts_and_no_rt_allocation() {
        let mut output = OutputParameters::new(&[metadata(7)], 128).unwrap();
        output.begin_block(128);
        let id = 7_u32;
        let mut index = -1;
        let raw = unsafe {
            output
                .pointer
                .addParameterData(&raw const id, &raw mut index)
        };
        let queue = unsafe { ComRef::<IParamValueQueue>::from_raw(raw) }
            .expect("known output queue pointer");
        let allocations = count_allocations(|| {
            for offset in (0..128).rev() {
                let mut point_index = -1;
                assert_eq!(
                    unsafe {
                        queue.addPoint(offset, f64::from(offset) / 128.0, &raw mut point_index)
                    },
                    kResultOk
                );
                assert_eq!(point_index, 0);
            }
            assert_eq!(
                unsafe { queue.addPoint(64, 0.5, &raw mut index) },
                kResultOk
            );
            output.capture_block();
        });
        assert_eq!(allocations, 0);
        assert_eq!(index, 64);
        assert_eq!(unsafe { queue.getPointCount() }, 128);
        assert_eq!(output.latest.as_slice(), &[Some(127.0 / 128.0)]);
        assert_eq!(output.pending.as_slice(), &[0]);
        assert_eq!(
            unsafe { queue.addPoint(-1, 0.5, &raw mut index) },
            kInvalidArgument
        );
        assert_eq!(
            unsafe { queue.addPoint(128, 0.5, &raw mut index) },
            kInvalidArgument
        );
        assert_eq!(
            unsafe { queue.addPoint(0, f64::NAN, &raw mut index) },
            kInvalidArgument
        );
        let mut offset = -1;
        let mut value = -1.0;
        assert_eq!(
            unsafe { queue.getPoint(64, &raw mut offset, &raw mut value) },
            kResultOk
        );
        assert_eq!((offset, value), (64, 0.5));
    }

    #[test]
    fn output_service_retries_latest_and_restore_discards_stale_values() {
        let mut output = OutputParameters::new(&[metadata(7)], 1).unwrap();
        output.latest[0] = Some(0.25);
        output.pending.push(0);
        assert!(output.service(|_, _| false).is_err());
        assert_eq!(output.latest.as_slice(), &[Some(0.25)]);
        assert_eq!(output.pending.as_slice(), &[0]);

        output.latest[0] = Some(0.75);
        let mut observed = None;
        output
            .service(|id, value| {
                observed = Some((id, value));
                true
            })
            .unwrap();
        assert_eq!(observed, Some((7, 0.75)));
        assert!(output.pending.is_empty());
        assert_eq!(output.latest.as_slice(), &[None]);

        output.latest[0] = Some(0.5);
        output.pending.push(0);
        output.discard();
        assert!(output.pending.is_empty());
        assert_eq!(output.latest.as_slice(), &[None]);
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(super) use crate::test_support::count_allocations;
