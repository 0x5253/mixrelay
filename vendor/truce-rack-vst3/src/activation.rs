//! VST3 activation ordering and latency snapshot publication.

use truce_rack_core::error::{Error, Result};
use vst3::{
    ComPtr,
    Steinberg::{
        Vst::{IAudioProcessor, IAudioProcessorTrait, IComponent, IComponentTrait, ProcessSetup},
        kResultOk,
    },
};

use crate::bus_layout::{self, NativeLayout};

/// Mutable lifecycle values shared by one deactivate/reactivate operation.
pub(super) struct RestartState<'a> {
    pub(super) setup: &'a mut ProcessSetup,
    pub(super) processing: &'a mut bool,
    pub(super) latency_frames: &'a mut Option<u32>,
}

/// Negotiate and activate one layout, publishing latency only after startup succeeds.
pub(super) fn activate(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    layout: &NativeLayout,
    setup: &mut ProcessSetup,
    latency_frames: &mut Option<u32>,
) -> Result<()> {
    *latency_frames = None;
    bus_layout::negotiate(component, processor, layout)?;

    // SAFETY: `setup` is a live mutable value for this synchronous call, and `processor`
    // is a retained COM interface. Activation is serialized on the wrapper lifecycle thread.
    if unsafe { processor.setupProcessing(&raw mut *setup) } != kResultOk {
        return Err(Error::Other(
            "IAudioProcessor::setupProcessing failed".into(),
        ));
    }
    bus_layout::activate_buses(component, layout, true)?;
    // SAFETY: `component` is retained for the duration of the call; this is the serialized
    // lifecycle thread, and the native component does not retain the scalar state argument.
    if unsafe { component.setActive(1) } != kResultOk {
        // SAFETY: same retained component and lifecycle thread as the activation call above.
        unsafe { component.setActive(0) };
        let _ = bus_layout::activate_buses(component, layout, false);
        return Err(Error::Other("IComponent::setActive(true) failed".into()));
    }

    // Steinberg's latency sequence is setActive(true), getLatencySamples(), then
    // setProcessing(true). Keep this native getter on the lifecycle thread, not process().
    // SAFETY: `processor` remains retained and the component is active after the successful
    // setActive call. The caller serializes native lifecycle operations.
    let latency = unsafe { processor.getLatencySamples() };
    // SAFETY: retained processor/component, serialized lifecycle thread, and no process call
    // begins until this synchronous activation method returns.
    if unsafe { processor.setProcessing(1) } != kResultOk {
        // SAFETY: rollback calls are synchronous on the same lifecycle thread with retained
        // interfaces; all configured buses are explicitly deactivated below.
        unsafe {
            processor.setProcessing(0);
            component.setActive(0);
        }
        let _ = bus_layout::activate_buses(component, layout, false);
        return Err(Error::Other(
            "IAudioProcessor::setProcessing(true) failed".into(),
        ));
    }

    *latency_frames = Some(latency);
    Ok(())
}

/// Stop processing and deactivate the configured buses on the lifecycle thread.
pub(super) fn deactivate(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    layout: &NativeLayout,
    processing: &mut bool,
) -> Result<()> {
    let mut failed = false;
    if *processing {
        // SAFETY: retained processor and serialized lifecycle thread; no process call runs while
        // the host owns the exclusive plug-in mutex.
        let stopped = unsafe { processor.setProcessing(0) } == kResultOk;
        *processing = !stopped;
        failed |= !stopped;
    }
    // Attempt every cleanup step even after one native failure, while preserving a failed status.
    // SAFETY: retained component, creator thread, and a layout previously negotiated at activation.
    failed |= unsafe { component.setActive(0) } != kResultOk;
    failed |= bus_layout::activate_buses(component, layout, false).is_err();
    if failed {
        return Err(Error::Other("VST3 deactivation failed".into()));
    }
    Ok(())
}

/// Restart an active instance after a `kLatencyChanged` request.
pub(super) fn restart(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    layout: &NativeLayout,
    state: &mut RestartState<'_>,
) -> Result<()> {
    *state.latency_frames = None;
    deactivate(component, processor, layout, state.processing)?;
    activate(
        component,
        processor,
        layout,
        state.setup,
        state.latency_frames,
    )?;
    *state.processing = true;
    Ok(())
}

#[cfg(test)]
mod tests;

/// Attempt shutdown of an active layout before initialized interfaces terminate.
pub(super) fn deactivate_before_terminate(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    layout: Option<&NativeLayout>,
    processing: &mut bool,
) {
    if let Some(layout) = layout {
        let _ = deactivate(component, processor, layout, processing);
    }
}
