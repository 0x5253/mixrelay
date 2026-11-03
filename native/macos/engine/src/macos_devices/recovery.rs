//! Reconcile desired selections against current presence without deleting graph intent.
use super::{
    Aggregate, Cache, Device, DeviceSet, MacDevices, Publication, Spec, aggregate_members,
    backend_error,
};
use mixrelay_audio_graph::DeviceSelector;
use mixrelay_audio_io::process::{InputActivity, TransferOutcome};
use mixrelay_audio_io::{AudioDeviceDescriptor, DeviceDirection, HostAudioBackendError};
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};

pub(super) fn offline_transfer(samples: usize, channels: usize) -> TransferOutcome {
    TransferOutcome {
        frames: u32::try_from(samples / channels.max(1)).unwrap_or(0),
        discontinuity: false,
        activity: InputActivity::Inactive,
        source_sample_faults: 0,
    }
}

struct Presence {
    online: BTreeMap<String, Spec>,
    offline: BTreeMap<String, Spec>,
}
impl Presence {
    fn observe(
        desired: &BTreeMap<String, Spec>,
        inventory: &[AudioDeviceDescriptor],
    ) -> Result<Self, HostAudioBackendError> {
        let mut presence = Self {
            online: BTreeMap::new(),
            offline: BTreeMap::new(),
        };
        for (id, spec) in desired {
            spec.native_uid().map_err(backend_error)?;
            if !(1..=64).contains(&spec.channels) {
                return Err(backend_error("invalid device channel count"));
            }
            let mut candidates = inventory.iter().filter(|device| match &spec.selector {
                DeviceSelector::PersistentUid { uid } => device.uid == *uid,
                DeviceSelector::DefaultInput => device.is_default_input,
                DeviceSelector::DefaultOutput => device.is_default_output,
            });
            let first = candidates.next();
            if candidates.next().is_some() {
                return Err(backend_error("ambiguous device selection"));
            }
            let found = first.is_some_and(|device| {
                if spec.input {
                    device.input_channels > 0
                } else {
                    device.output_channels > 0
                }
            });
            if found {
                // A present device with an invalid channel request is an error,
                // not evidence that the device was unplugged.
                spec.selector.resolve(
                    inventory,
                    if spec.input {
                        DeviceDirection::Input
                    } else {
                        DeviceDirection::Output
                    },
                    spec.channels,
                )?;
                presence.online.insert(id.clone(), spec.clone());
            } else {
                presence.offline.insert(id.clone(), spec.clone());
            }
        }
        Ok(presence)
    }
}

impl MacDevices {
    pub(super) fn prepare_devices(
        &self,
        cache: &mut Cache,
        desired: &BTreeMap<String, Spec>,
        publication: Publication,
    ) -> Result<(), HostAudioBackendError> {
        // Inventory errors abort preparation; an empty successful inventory is
        // different and leaves every desired selection explicitly offline.
        let (DeviceSet(devices, offline), next_cache) =
            self.prepare_device_set(cache, desired.clone(), self.format.current())?;
        self.require_preparation()?;
        if unchanged(cache, desired, &devices) {
            return Ok(());
        }
        let online_bindings = devices.len();
        let offline_bindings = offline.len();
        self.handoff
            .publish(publication, DeviceSet(devices, offline))
            .map_err(|_| backend_error("CoreAudio resource publication failed"))?;
        *cache = next_cache;
        tracing::info!(
            online_bindings,
            offline_bindings,
            "CoreAudio device presence reconciled"
        );
        Ok(())
    }

    pub(super) fn prepare_device_set(
        &self,
        cache: &Cache,
        desired: BTreeMap<String, Spec>,
        format: mixrelay_audio_graph::RenderFormat,
    ) -> Result<(DeviceSet, Cache), HostAudioBackendError> {
        self.require_preparation()?;
        let selectors: Vec<_> = desired.values().map(|spec| &spec.selector).collect();
        let inventory = self.host.inventory(&selectors)?;
        // Selection may wait for CoreAudio IPC; do not open a replacement after Quit.
        self.require_preparation()?;
        let presence = Presence::observe(&desired, &inventory)?;
        self.require_preparation()?;
        let group = prepare_group(cache, &presence.online, &self.host)?;
        self.require_preparation()?;
        let devices = self.open_present(cache, &presence.online, group.as_ref(), format)?;
        let next_cache = desired
            .into_iter()
            .map(|(id, spec)| {
                let owner = devices.get(&id).map_or_else(Weak::new, Arc::downgrade);
                (id, (spec, owner))
            })
            .collect();
        Ok((DeviceSet(devices, presence.offline), next_cache))
    }

    fn open_present(
        &self,
        cache: &Cache,
        online: &BTreeMap<String, Spec>,
        group: Option<&Arc<Aggregate>>,
        format: mixrelay_audio_graph::RenderFormat,
    ) -> Result<BTreeMap<String, Arc<Device>>, HostAudioBackendError> {
        let mut devices = BTreeMap::new();
        let has_inputs = online.values().any(|spec| spec.input);
        for (id, spec) in online {
            self.require_preparation()?;
            let selected = (matches!(spec.selector, DeviceSelector::PersistentUid { .. })
                && !super::spec::is_virtual_selection(&spec.selector))
            .then(|| group.cloned())
            .flatten();
            let existing = cache
                .get(id)
                .filter(|(old, _)| old == spec)
                .and_then(|(_, weak)| weak.upgrade())
                .filter(|device| same_group(device.native.group.as_ref(), selected.as_ref()));
            let output_only = !spec.input && !has_inputs;
            let existing = existing.filter(|device| {
                device
                    .native
                    .output_only
                    .load(std::sync::atomic::Ordering::Acquire)
                    == output_only
            });
            let device = match existing {
                Some(device) => device,
                None => Arc::new(Device::open_with_group(
                    spec,
                    format.sample_rate,
                    selected,
                    super::device::CallbackConfig {
                        buffer_frames: format.block_frames,
                        output_only,
                    },
                    self.host.clone(),
                )?),
            };
            devices.insert(id.clone(), device);
        }
        Ok(devices)
    }
}

fn same_group(left: Option<&Arc<Aggregate>>, right: Option<&Arc<Aggregate>>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => Arc::ptr_eq(left, right),
        (None, None) => true,
        _ => false,
    }
}

fn prepare_group(
    cache: &Cache,
    online: &BTreeMap<String, Spec>,
    host: &Arc<dyn super::host::Host>,
) -> Result<Option<Arc<Aggregate>>, HostAudioBackendError> {
    let members = aggregate_members(online);
    if let Some(group) = cache
        .values()
        .filter_map(|(_, weak)| weak.upgrade())
        .find_map(|device| {
            device
                .native
                .group
                .clone()
                .filter(|group| same_members(&group.members, &members) && group.ready())
        })
    {
        return Ok(Some(group));
    }
    Aggregate::create(members, host).map_err(backend_error)
}

/// A graph-local ordering preference does not change a native clock group.
/// Keep its established primary when every physical member is still required.
fn same_members(current: &[String], requested: &[String]) -> bool {
    if current == requested {
        return true;
    }
    if current.len() != requested.len() {
        return false;
    }
    let mut current: Vec<_> = current.iter().collect();
    let mut requested: Vec<_> = requested.iter().collect();
    current.sort_unstable();
    requested.sort_unstable();
    current == requested
}

fn unchanged(
    cache: &Cache,
    desired: &BTreeMap<String, Spec>,
    devices: &BTreeMap<String, Arc<Device>>,
) -> bool {
    cache.len() == desired.len()
        && desired.iter().all(|(id, spec)| {
            cache.get(id).is_some_and(|(old, weak)| {
                old == spec
                    && match (weak.upgrade(), devices.get(id)) {
                        (Some(old), Some(new)) => Arc::ptr_eq(&old, new),
                        (None, None) => true,
                        _ => false,
                    }
            })
        })
}

#[cfg(test)]
mod tests;
