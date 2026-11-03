//! A single native callback owns the hardware graph. The control
//! supervisor prepares channel maps and drains publication before reclaiming
//! the borrowed Rust renderer. Independent clocks retain their explicit bridge.
use super::{
    DeviceSet,
    device::Device,
    ffi::{mr_device_io_clock_discontinuities, mr_device_io_output_epoch},
};
use mixrelay_audio_engine::{ExternalBinding, RenderPlan};
use std::{
    collections::BTreeMap,
    ffi::c_void,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};
mod metrics;
use metrics::CallbackMetrics;

pub(crate) fn next_epoch() -> u64 {
    super::bridge::NEXT_DEVICE_EPOCH.fetch_add(1, Ordering::Relaxed)
}

#[repr(C)]
pub(crate) struct Descriptor {
    pub(crate) context: *mut c_void,
    pub(crate) render: unsafe extern "C" fn(
        *mut c_void,
        *const c_void,
        *mut c_void,
        u32,
        *const mixrelay_macos_transport::device_io::IoTiming,
    ),
    pub(crate) sample_rate: u32,
}

#[derive(Default)]
pub(super) struct RenderSlot {
    pub(super) activity: Arc<super::activity::BridgeActivity>,
    enabled: AtomicBool,
    descriptor: AtomicPtr<Descriptor>,
    readers: AtomicU32,
    rendering: AtomicBool,
    metrics: CallbackMetrics,
    pub(super) generation: AtomicU64,
    pub(super) reconnect: AtomicBool,
}
impl RenderSlot {
    pub(super) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
    fn publish(&self, descriptor: *mut Descriptor) {
        self.descriptor.store(descriptor, Ordering::SeqCst);
    }
    fn drained(&self) -> bool {
        self.readers.load(Ordering::SeqCst) == 0
    }
    fn finish(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        self.enabled.store(false, Ordering::Release);
    }
}

pub(super) unsafe extern "C" fn dispatch(
    context: *mut c_void,
    input: *const c_void,
    output: *mut c_void,
    frames: u32,
    timing: *const mixrelay_macos_transport::device_io::IoTiming,
) -> bool {
    // SAFETY: Native owns this stable box until CoreAudio unregister is acknowledged.
    let slot = unsafe { &*context.cast::<RenderSlot>() };
    if !slot.enabled() {
        // The clock-boundary hook acknowledges OFF/ON before admitting native PCM.
        return false;
    }
    slot.readers.fetch_add(1, Ordering::SeqCst);
    let descriptor = slot.descriptor.load(Ordering::SeqCst);
    // Never wait on an audio callback, even for an unexpected concurrent vendor
    // invocation. This also enforces the exclusive Rust mutable-renderer contract.
    if !descriptor.is_null() {
        if slot
            .rendering
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            let descriptor = unsafe { &*descriptor };
            let started = std::time::Instant::now();
            unsafe {
                (descriptor.render)(descriptor.context, input, output, frames, timing);
            }
            slot.metrics
                .record(started.elapsed(), frames, descriptor.sample_rate);
            slot.rendering.store(false, Ordering::Release);
        } else {
            slot.metrics.missed();
        }
    }
    slot.readers.fetch_sub(1, Ordering::SeqCst);
    true
}

pub(crate) struct Binding {
    pub(crate) id: String,
    pub(crate) input: bool,
    /// No native ABL storage exists for an offline or unprepared disabled binding.
    pub(crate) offset: Option<usize>,
    pub(crate) channels: usize,
}

impl Binding {
    fn prepare(
        devices: &DeviceSet,
        binding: &ExternalBinding,
        selection: BindingSelection,
    ) -> Option<Self> {
        let BindingSelection {
            input,
            enabled,
            clock_id,
            native_rate,
        } = selection;
        let binding_id = binding.node_id().as_str();
        let id = devices.binding_key(binding_id, input).unwrap_or(binding_id);
        let offset = if devices.is_offline(id) || (!enabled && !devices.0.contains_key(id)) {
            None
        } else {
            let device = devices.0.get(id)?;
            if !device.ready()
                || device.input != input
                || device.clock_identity.load(Ordering::Acquire) != clock_id
                || device.native_rate.load(Ordering::Acquire) != native_rate
            {
                return None;
            }
            let resolved = device.native.host.direct_binding(&device.native)?;
            if resolved.device != clock_id {
                return None;
            }
            Some(resolved.offset as usize)
        };
        Some(Self {
            id: binding_id.to_owned(),
            input,
            offset,
            channels: usize::from(binding.channels()),
        })
    }
}

#[derive(Clone, Copy)]
struct BindingSelection {
    input: bool,
    enabled: bool,
    clock_id: u32,
    native_rate: u32,
}

pub(crate) struct DirectPlan {
    pub(super) master: Arc<Device>,
    pub(super) owners: Vec<Arc<Device>>,
    pub(super) latency_bindings: Vec<(String, Arc<Device>)>,
    pub(crate) bindings: Vec<Binding>,
    pub(crate) bridged: DeviceSet,
    pub(crate) clock_id: u32,
}

impl DirectPlan {
    /// The master's active I/O direction supplies both sample and host time.
    pub(crate) fn cycle_time(
        &self,
        timing: &mixrelay_macos_transport::device_io::IoTiming,
    ) -> mixrelay_macos_transport::device_io::IoTimestamp {
        if self.master.input {
            timing.input
        } else {
            timing.output
        }
    }

    /// The direct master's current callback label, its mean frame's host time. The
    /// bridged transfers that callback renders pass at it, like its native transfers.
    pub(crate) fn callback_host(&self) -> u64 {
        self.master.callback_host()
    }
    /// Control-side logging only; callback counters never take the logging path.
    pub(crate) fn report_execution(&self, elapsed: Duration) {
        self.master
            .native
            .render
            .metrics
            .report(self.clock_id, elapsed);
    }

    /// Control only. The master renders synchronously; independent
    /// devices keep their own clock-crossing queues, never native ABL aliases.
    /// Inventory-confirmed absent bindings have no native storage or owner. No HAL
    /// shared-memory or app-capture timeline is silently relabelled as this `IOProc`.
    pub(crate) fn prepare(devices: &DeviceSet, plan: &RenderPlan) -> Option<Self> {
        let master = devices
            .0
            .values()
            .filter(|device| device.ready())
            .min_by_key(|device| device.input)?
            .clone();
        let clock_id = master.clock_identity.load(Ordering::Acquire);
        let native_rate = master.native_rate.load(Ordering::Acquire);
        if clock_id == 0 || native_rate != plan.format().sample_rate {
            return None;
        }
        if independent_inputs(devices, plan, clock_id) {
            return None;
        }
        let mut bindings = Vec::new();
        let mut latency_bindings = Vec::new();
        let mut bridged = DeviceSet::default();
        let mut owners = BTreeMap::from([(master.identity, Arc::clone(&master))]);
        for (input, list) in [
            (true, plan.source_bindings()),
            (false, plan.sink_bindings()),
        ] {
            for binding in list {
                let binding_id = binding.node_id().as_str();
                let id = devices.binding_key(binding_id, input).unwrap_or(binding_id);
                if input
                    && binding.virtual_endpoint_id().is_none()
                    && !devices.0.contains_key(id)
                    && !devices.1.contains_key(id)
                {
                    continue;
                }
                let enabled = plan
                    .controls()
                    .snapshot(binding.node_id())
                    .is_none_or(|(control, _)| control.enabled);
                if let Some(device) = devices.0.get(id)
                    && (device.clock_identity.load(Ordering::Acquire) != clock_id
                        || device.native_rate.load(Ordering::Acquire) != native_rate)
                {
                    bridged.0.insert(id.to_owned(), Arc::clone(device));
                    continue;
                }
                let map = Binding::prepare(
                    devices,
                    binding,
                    BindingSelection {
                        input,
                        enabled,
                        clock_id,
                        native_rate,
                    },
                )?;
                if map.offset.is_some() {
                    let device = devices.0.get(id)?;
                    owners.insert(device.identity, Arc::clone(device));
                    latency_bindings.push((id.to_owned(), Arc::clone(device)));
                }
                bindings.push(map);
            }
        }
        if bindings.is_empty() {
            return None;
        }
        Some(Self {
            master,
            owners: owners.into_values().collect(),
            latency_bindings,
            bindings,
            bridged,
            clock_id,
        })
    }

    pub(crate) fn healthy(&self) -> bool {
        self.owners.iter().all(|device| {
            !device.failed.load(Ordering::Acquire)
                && device.clock_identity.load(Ordering::Acquire) == self.clock_id
        })
    }

    pub(crate) fn discontinuities(&self) -> u64 {
        // Atomic observation only; unlike device properties this is callback-safe.
        unsafe { mr_device_io_clock_discontinuities(self.master.native.pointer.as_ptr()) }
    }

    pub(crate) fn request_reconnect(&self) {
        for owner in &self.owners {
            owner.failed.store(true, Ordering::Release);
            owner.native.render.reconnect.store(true, Ordering::Release);
            if let Some(worker) = &owner.worker {
                worker.thread().unpark();
            }
        }
    }

    /// # Safety
    /// Descriptor and its exclusive renderer must remain valid and untouched
    /// until this guard drops. The `IOProc` is its only mutable caller.
    pub(crate) unsafe fn install<'a>(&'a self, descriptor: &'a Descriptor) -> Registration<'a> {
        // Only direct owners share the callback timeline. Existing independent
        // converters stay independent across HAL capture open/close activity.
        for device in self.bridged.0.values() {
            device.clock_aligned.store(false, Ordering::Release);
        }
        for device in &self.owners {
            device.native.render.enabled.store(true, Ordering::Release);
        }
        self.master
            .native
            .render
            .publish(std::ptr::from_ref(descriptor).cast_mut());
        Registration { plan: self }
    }
}

pub(crate) struct Registration<'a> {
    plan: &'a DirectPlan,
}
impl Registration<'_> {
    /// Remove publication before releasing the callback-borrowed renderer storage.
    fn drain(&self) {
        self.plan.master.native.render.publish(std::ptr::null_mut());
        while !self.plan.master.native.render.drained() {
            thread::sleep(Duration::from_micros(100));
        }
    }
}
impl Drop for Registration<'_> {
    fn drop(&mut self) {
        // Remove publication first; a timeout is NOT permission to free a
        // renderer still reachable by CoreAudio. Waiting occurs on control only.
        self.drain();
        for device in &self.plan.owners {
            device.ready_rate.store(0, Ordering::Release);
            // Invalidate pre-direct PCM BEFORE exposing the ring consumer again.
            // This is an atomic epoch publication, not a cross-thread cursor reset.
            unsafe {
                mr_device_io_output_epoch(device.native.pointer.as_ptr(), next_epoch());
            }
            // Publish the EXIT, not entry: the supervisor may have sampled the
            // old mode just before entry, so an entry generation can be consumed
            // before the direct lifetime even starts.
            device.native.render.finish();
            if let Some(worker) = &device.worker {
                worker.thread().unpark();
            }
        }
    }
}

fn independent_inputs(devices: &DeviceSet, plan: &RenderPlan, clock_id: u32) -> bool {
    // Native buffers already share acquisition coordinates only when every
    // active input belongs to this exact IOProc clock. Even a single independent
    // input needs the worker's time-addressed queue instead of an ordinary FIFO.
    plan.source_bindings()
        .iter()
        .filter(|binding| {
            plan.controls()
                .snapshot(binding.node_id())
                .is_none_or(|(control, _)| control.enabled)
        })
        .any(|binding| {
            devices
                .binding_key(binding.node_id().as_str(), true)
                .and_then(|id| devices.0.get(id))
                .is_none_or(|device| device.clock_identity.load(Ordering::Acquire) != clock_id)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_output_plan() -> (mixrelay_audio_graph::GraphDocument, RenderPlan) {
        use mixrelay_audio_graph::{
            CompileContext, CompileLimits, GraphCompiler, GraphDocument, RenderFormat,
        };
        let document: GraphDocument = serde_json::from_value(serde_json::json!({
            "schemaVersion":2,"revision":1,"nodes":[
                {"id":"input","type":"audio_device_input","config":{"selector":{"match":"default_input"},"outputChannels":2,"gainDb":0}},
                {"id":"output","type":"audio_device_output","config":{"selector":{"match":"default_output"},"inputChannels":2}}
            ],"edges":[{"id":"wire","source":{"nodeId":"input","portId":"audio"},"destination":{"nodeId":"output","portId":"audio"}}]
        })).unwrap();
        let execution = GraphCompiler::default()
            .compile(
                &document,
                CompileContext {
                    format: RenderFormat {
                        sample_rate: 48_000,
                        block_frames: 32,
                    },
                    limits: CompileLimits::default(),
                },
            )
            .unwrap();
        let plan = RenderPlan::prepare(&execution).unwrap();
        (document, plan)
    }

    #[test]
    fn one_independent_input_requires_time_addressed_worker_reads() {
        let (_, plan) = input_output_plan();
        let (input, _context) = super::super::tests::synthetic_device(2, 48_000, 48_000, true);
        input.clock_identity.store(11, Ordering::Release);
        let devices = DeviceSet(
            BTreeMap::from([("input".into(), Arc::clone(&input))]),
            BTreeMap::new(),
        );
        assert!(independent_inputs(&devices, &plan, 22));
        input.clock_identity.store(22, Ordering::Release);
        assert!(!independent_inputs(&devices, &plan, 22));
    }

    #[test]
    fn confirmed_offline_bindings_need_no_native_mapping_but_unknown_bindings_are_rejected() {
        let (document, plan) = input_output_plan();
        let devices = DeviceSet(BTreeMap::new(), super::super::spec::specs(&document));
        for (input, list) in [
            (true, plan.source_bindings()),
            (false, plan.sink_bindings()),
        ] {
            for binding in list {
                let selection = || BindingSelection {
                    input,
                    enabled: true,
                    clock_id: 7,
                    native_rate: 48_000,
                };
                let prepared = Binding::prepare(&devices, binding, selection()).unwrap();
                assert_eq!(
                    (prepared.input, prepared.offset, prepared.channels),
                    (input, None, 2)
                );
                assert!(Binding::prepare(&DeviceSet::default(), binding, selection()).is_none());
                let disabled = BindingSelection {
                    enabled: false,
                    ..selection()
                };
                let inactive = Binding::prepare(&DeviceSet::default(), binding, disabled).unwrap();
                assert_eq!(inactive.offset, None);
                assert_eq!(inactive.channels, 2);
            }
        }
        assert!(
            DirectPlan::prepare(&devices, &plan).is_none(),
            "offline-only sets cannot invent a callback clock"
        );
    }

    struct Probe {
        entered: AtomicBool,
        release: AtomicBool,
        calls: AtomicU64,
    }
    unsafe extern "C" fn held_render(
        context: *mut c_void,
        _: *const c_void,
        _: *mut c_void,
        _: u32,
        _: *const mixrelay_macos_transport::device_io::IoTiming,
    ) {
        let probe = unsafe { &*context.cast::<Probe>() };
        probe.calls.fetch_add(1, Ordering::Relaxed);
        probe.entered.store(true, Ordering::Release);
        // Deliberately hold a TEST renderer to prove control cannot reclaim it.
        while !probe.release.load(Ordering::Acquire) {
            thread::yield_now();
        }
    }

    #[test]
    fn removal_drains_in_flight_renderer_and_excludes_concurrent_mutation() {
        let slot = Arc::new(RenderSlot::default());
        let mut probe = Box::new(Probe {
            entered: AtomicBool::new(false),
            release: AtomicBool::new(false),
            calls: AtomicU64::new(0),
        });
        let mut descriptor = Box::new(Descriptor {
            context: (&raw mut *probe).cast(),
            render: held_render,
            sample_rate: 48_000,
        });
        slot.enabled.store(true, Ordering::Release);
        slot.publish(&raw mut *descriptor);
        let other = slot.clone();
        let callback = thread::spawn(move || unsafe {
            dispatch(
                Arc::as_ptr(&other).cast_mut().cast(),
                std::ptr::null(),
                std::ptr::null_mut(),
                32,
                std::ptr::null(),
            )
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !probe.entered.load(Ordering::Acquire) {
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
        // A second native invocation must not form another &mut Renderer.
        unsafe {
            dispatch(
                Arc::as_ptr(&slot).cast_mut().cast(),
                std::ptr::null(),
                std::ptr::null_mut(),
                32,
                std::ptr::null(),
            );
        }
        assert_eq!(probe.calls.load(Ordering::Acquire), 1);
        assert_eq!(slot.metrics.missed_count(), 1);
        slot.publish(std::ptr::null_mut());
        assert!(!slot.drained());
        probe.release.store(true, Ordering::Release);
        assert!(callback.join().unwrap());
        assert!(slot.drained());
        drop(descriptor);
        drop(probe);
        for _ in 0..10000 {
            assert!(unsafe {
                dispatch(
                    Arc::as_ptr(&slot).cast_mut().cast(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    32,
                    std::ptr::null(),
                )
            });
        }
        assert!(slot.drained());
        assert_eq!(slot.metrics.missed_count(), 1);
    }

    #[test]
    fn completed_direct_lifetime_is_observable_without_observing_enabled() {
        let slot = RenderSlot::default();
        let observed = slot.generation.load(Ordering::Acquire);
        slot.enabled.store(true, Ordering::Release);
        slot.finish();
        assert!(!slot.enabled());
        assert_ne!(observed, slot.generation.load(Ordering::Acquire));
    }
}
