//! Device inventory selection over platform-supplied observations.
use crate::{AudioDeviceDescriptor, DeviceSelector, HostAudioBackendError};

#[derive(Debug, Clone)]
pub struct DeviceObservation {
    pub descriptor: AudioDeviceDescriptor,
    pub alive: bool,
    pub hidden: bool,
}

pub trait DeviceInventory {
    type Id: Copy + Ord;
    /// # Errors
    /// Returns a failed native inventory read.
    fn device_ids(&self) -> Result<Vec<Self::Id>, HostAudioBackendError>;
    fn default_input(&self) -> Option<Self::Id>;
    fn default_output(&self) -> Option<Self::Id>;
    fn device(&self, id: Self::Id) -> Option<DeviceObservation>;
    /// # Errors
    /// Returns invalid identifiers or native identity lookup failures.
    fn resolve_uid(&self, uid: &str) -> Result<Option<Self::Id>, HostAudioBackendError>;
}

#[derive(Default)]
struct SelectedDevice {
    explicit: bool,
    default_input: bool,
    default_output: bool,
}

/// Resolves only requested device identities, preserving explicit private selections.
/// # Errors
/// Returns identity lookup failures from the inventory port.
pub fn selected_inventory<'a>(
    reader: &impl DeviceInventory,
    selectors: impl IntoIterator<Item = &'a DeviceSelector>,
) -> Result<Vec<AudioDeviceDescriptor>, HostAudioBackendError> {
    // Resolve identities first so explicit/default aliases observe geometry once.
    let mut selected = std::collections::BTreeMap::<_, SelectedDevice>::new();
    for selector in selectors {
        let id = match selector {
            DeviceSelector::PersistentUid { uid } => reader.resolve_uid(uid)?,
            DeviceSelector::DefaultInput => reader.default_input(),
            DeviceSelector::DefaultOutput => reader.default_output(),
        };
        if let Some(id) = id {
            let flags = selected.entry(id).or_default();
            match selector {
                DeviceSelector::PersistentUid { .. } => flags.explicit = true,
                DeviceSelector::DefaultInput => flags.default_input = true,
                DeviceSelector::DefaultOutput => flags.default_output = true,
            }
        }
    }
    Ok(selected
        .into_iter()
        .filter_map(|(id, selection)| {
            let info = reader.device(id)?;
            let mut device = info.descriptor;
            if !info.alive || (info.hidden && !selection.explicit) || device.uid.is_empty() {
                return None;
            }
            device.is_default_input = selection.default_input;
            device.is_default_output = selection.default_output;
            Some(device)
        })
        .collect())
}

/// Filters visible, alive devices and annotates current defaults.
/// # Errors
/// Returns a failure to enumerate native device identities.
pub fn inventory(
    reader: &impl DeviceInventory,
) -> Result<Vec<AudioDeviceDescriptor>, HostAudioBackendError> {
    let ids = reader.device_ids()?;
    let default_input = reader.default_input();
    let default_output = reader.default_output();
    let mut devices = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(info) = reader.device(id) else {
            continue;
        };
        let mut device = info.descriptor;
        if !info.alive
            || info.hidden
            || device.uid.is_empty()
            || device.display_name.is_empty()
            || (device.input_channels == 0 && device.output_channels == 0)
        {
            continue;
        }
        device.is_default_input = default_input == Some(id);
        device.is_default_output = default_output == Some(id);
        devices.push(device);
    }
    Ok(devices)
}

#[cfg(test)]
mod tests;
