use super::{
    Descriptor, DirectPlan,
    renderer::{Renderer, render},
};
use crate::macos_devices::direct::Registration;
use std::{cell::UnsafeCell, pin::Pin};

/// Owns the only renderer storage and its descriptor. Pinning both allocations
/// keeps native addresses stable even if this control-side owner moves.
pub(super) struct PreparedRenderer<'state> {
    descriptor: Pin<Box<Descriptor>>,
    state: Pin<Box<UnsafeCell<Renderer<'state>>>>,
}

impl<'state> PreparedRenderer<'state> {
    pub(super) fn new(renderer: Renderer<'state>) -> Self {
        let sample_rate = renderer.processor.plan().format().sample_rate;
        let state = Box::pin(UnsafeCell::new(renderer));
        let descriptor = Box::pin(Descriptor {
            context: state.as_ref().get_ref().get().cast(),
            render,
            sample_rate,
        });
        Self { descriptor, state }
    }

    /// Publication borrows this owner exclusively until the registration has
    /// drained. No safe caller can inspect, mutate or reclaim its renderer then.
    pub(super) fn install<'published>(
        &'published mut self,
        plan: &'published DirectPlan,
    ) -> Registration<'published> {
        let descriptor = self.descriptor.as_ref().get_ref();
        debug_assert_eq!(
            descriptor.context,
            self.state.as_ref().get_ref().get().cast()
        );
        // SAFETY: both pinned allocations outlive the returned exclusive owner
        // borrow. RenderSlot serializes callbacks; Registration withdraws
        // publication and drains readers before accessing UnsafeCell or dropping.
        unsafe { plan.install(descriptor) }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        buffers::Buffers,
        callback_io::CallbackIo,
        supervision::RenderSignals,
        tests::{processor_at, status, stereo_bindings, stereo_list, unclocked},
    };
    use super::*;
    use crate::macos_devices::{
        DeviceSet,
        callback_fixture::{CallbackTime, NativeOwner, direct_plan},
        direct::Binding,
    };
    use mixrelay_audio_io::{
        process::{ClockDomain, CycleError, ProcessCycle},
        session::ClockAuthority,
    };
    use mixrelay_realtime::Timebase;
    use mixrelay_render::{BlockProcessor, PeakMeterBank, ProcessBlockError};
    use std::sync::atomic::Ordering;
    use std::{sync::Arc, time::Duration};

    fn invoke_unpublished(owner: &PreparedRenderer<'_>) -> [f32; 64] {
        let mut input = [0.25_f32; 64];
        let mut output = [0.0; 64];
        let source = stereo_list(&mut input);
        let mut sink = stereo_list(&mut output);
        let descriptor = owner.descriptor.as_ref().get_ref();
        // SAFETY: the fixture owns an unpublished, non-Sync renderer. Both ABLs
        // retain exactly 32 stereo frames through this sole synchronous invocation;
        // the owner pins its context until after the invocation returns.
        unsafe {
            (descriptor.render)(
                descriptor.context,
                (&raw const source).cast(),
                (&raw mut sink).cast(),
                32,
                std::ptr::null(),
            );
        }
        output
    }

    #[test]
    fn moving_the_prepared_owner_keeps_native_state_and_descriptor_stable() {
        let mut processor = processor_at(32, 48_000);
        let bindings = stereo_bindings();
        let status = status();
        let signals = RenderSignals::default();
        let owner =
            PreparedRenderer::new(unclocked(&mut processor, &bindings, &status, &signals, 1));
        let state_address = owner.state.as_ref().get_ref().get();
        let descriptor_address = std::ptr::from_ref(owner.descriptor.as_ref().get_ref());
        let mut moved = vec![owner];
        let owner = moved.pop().unwrap();
        assert_eq!(owner.state.as_ref().get_ref().get(), state_address);
        assert_eq!(
            std::ptr::from_ref(owner.descriptor.as_ref().get_ref()),
            descriptor_address
        );
        assert_eq!(
            invoke_unpublished(&owner).map(f32::to_bits),
            [0.25_f32.to_bits(); 64]
        );
        assert_eq!(signals.progress.load(Ordering::Acquire), 1);
        assert!(!signals.failed.load(Ordering::Acquire));
        drop(owner);
        assert_eq!(processor.plan().format().sample_rate, 48_000);
    }

    fn assert_acquisition_time_was_observed(processor: &mut BlockProcessor) {
        let mut bindings = stereo_bindings();
        for binding in &mut bindings {
            binding.offset = None;
        }
        let mut io = CallbackIo {
            buffers: Buffers::empty(),
            bindings: &bindings,
            offset: 0,
            bridged: None,
        };
        let next = ProcessCycle {
            clock: ClockDomain {
                authority: ClockAuthority::Device,
                id: 1,
                epoch: 19,
            },
            sample_rate: 48_000,
            frames: 32,
            first_frame: 48_128,
            monotonic_ns: Some(999_000_000),
            discontinuity: false,
        };
        assert_eq!(
            processor.process_external_cycle(next, &mut io),
            Err(ProcessBlockError::InvalidCycle(
                CycleError::TimestampRegressed
            ))
        );
    }

    #[test]
    fn input_master_keeps_native_pcm_and_position_without_an_output_timestamp() {
        let input = NativeOwner::new(2, 48_000, true);
        let mut plan = direct_plan("input", "input", &input, DeviceSet::default());
        plan.bindings.push(Binding {
            id: "output".into(),
            input: false,
            offset: None,
            channels: 2,
        });
        let prepared = processor_at(32, 48_000);
        let meters = Arc::new(PeakMeterBank::new(8, 2));
        let output_meter = prepared
            .plan()
            .meter_bindings()
            .iter()
            .position(|binding| binding.node_id().as_str() == "output")
            .unwrap();
        let mut processor = Box::new(
            BlockProcessor::with_peak_meters(Arc::clone(prepared.plan()), Arc::clone(&meters))
                .unwrap(),
        );
        let status = status();
        let signals = RenderSignals::default();
        let timebase = Timebase::current();
        let mut owner = PreparedRenderer::new(Renderer {
            timebase,
            epoch: 19,
            ..Renderer::for_plan(&plan, &mut processor, &status, &signals)
        });
        let installed = owner.install(&plan);
        for block in 0_u32..4 {
            let host = timebase.ticks(Duration::from_nanos(
                1_000_000_000 + u64::from(block) * 32 * 1_000_000_000 / 48_000,
            ));
            input.callback(
                &mut [0.25; 64],
                CallbackTime {
                    now: host,
                    host,
                    sample: 48_000.0 + f64::from(block * 32),
                    rate_scalar: 1.0,
                },
            );
            assert_eq!(meters.take_peaks(1, output_meter, 2), [0.25, 0.25]);
        }
        drop(installed);
        // SAFETY: withdrawing the registration drained every callback; this sole
        // control-side observer holds the unpublished owner until the read ends.
        let renderer = unsafe { &*owner.state.as_ref().get_ref().get() };
        assert_eq!(renderer.position, Some(48_128));
        assert_eq!(renderer.epoch, 19);
        assert_eq!(signals.progress.load(Ordering::Acquire), 4);
        assert!(!signals.failed.load(Ordering::Acquire));
        assert!(!signals.invalid.load(Ordering::Acquire));
        drop(owner);
        assert_acquisition_time_was_observed(&mut processor);
    }
}
