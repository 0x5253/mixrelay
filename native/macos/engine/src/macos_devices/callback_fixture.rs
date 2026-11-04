//! In-process native owners served by the production callback bridge, without `CoreAudio`.
//! Route tests drive their `IOProc` with explicit host timestamps.
use super::{
    DeviceSet, MacDevices,
    device::Device,
    device_fixture::synthetic_device,
    direct::{Binding, DirectPlan},
    spec::Spec,
    worker::callback::CallbackBridge,
};
use mixrelay_audio_graph::DeviceSelector;
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering},
};

pub(crate) use super::callback_invoke::{CallbackTime, invoke};

pub(crate) struct NativeOwner {
    device: Arc<Device>,
    context: super::worker::WorkerContext,
}

impl NativeOwner {
    pub(crate) fn new(channels: u16, rate: u32, input: bool) -> Self {
        Self::at_rates(channels, rate, rate, input)
    }

    pub(crate) fn at_rates(channels: u16, graph_rate: u32, native_rate: u32, input: bool) -> Self {
        let (device, context) = synthetic_device(channels, graph_rate, native_rate, input);
        Self { device, context }
    }

    /// Publishes the converter the connected device worker would, at the current clock alignment.
    #[must_use]
    pub(crate) fn serve(&self) -> impl Sized + '_ {
        CallbackBridge::new(
            &self.context,
            self.device.native_rate.load(Ordering::Acquire),
        )
        .expect("synthetic callback bridge")
    }

    /// One `IOProc` invocation over interleaved PCM: read for input, written for output.
    pub(crate) fn callback(&self, pcm: &mut [f32], time: CallbackTime) {
        // SAFETY: no OS registration exists, so this fixture is the owner's only serialized
        // callback, and its admitted layout has exactly the device's channel count.
        unsafe {
            invoke(
                self.context.native.pointer.as_ptr(),
                self.device.input,
                self.device.channels,
                pcm,
                time,
            );
        }
    }
}

/// The active set and the reporter's cache, keyed by graph binding id as preparation keys them.
pub(crate) fn publish(devices: &MacDevices, owners: &[(&str, &NativeOwner)]) -> DeviceSet {
    let mut cache = devices.cache.lock().expect("device cache");
    let mut active = BTreeMap::new();
    for (id, owner) in owners {
        let spec = Spec {
            selector: DeviceSelector::PersistentUid {
                uid: "synthetic-no-device".into(),
            },
            input: owner.device.input,
            channels: u16::try_from(owner.device.channels).expect("channel count"),
        };
        cache.insert((*id).into(), (spec, Arc::downgrade(&owner.device)));
        active.insert((*id).into(), Arc::clone(&owner.device));
    }
    DeviceSet(active, BTreeMap::new())
}

/// A direct plan whose `master` callback renders its one graph binding `node` (keyed
/// `key` in the device set) in place, while `bridged` devices keep their converters.
pub(crate) fn direct_plan(
    node: &str,
    key: &str,
    master: &NativeOwner,
    bridged: DeviceSet,
) -> DirectPlan {
    let device = &master.device;
    DirectPlan {
        master: Arc::clone(device),
        owners: vec![Arc::clone(device)],
        latency_bindings: vec![(key.into(), Arc::clone(device))],
        bindings: vec![Binding {
            id: node.into(),
            input: device.input,
            offset: Some(0),
            channels: device.channels,
        }],
        bridged,
        clock_id: 1,
    }
}
