//! `CoreAudio` discovery entry points backed by the portable inventory selection service.
use mixrelay_audio_io::{AudioDeviceDescriptor, DeviceSelector, HostAudioBackendError};
#[path = "coreaudio.rs"]
mod coreaudio;

/// Reads only the selected device identities, without enumerating other devices.
///
/// Default aliases and repeated UIDs share one descriptor. Explicit UIDs may
/// select process-private devices; hidden devices are not selected by a default
/// alias alone. Missing devices are omitted without substituting another device.
/// Like [`devices`], this performs synchronous control-thread OS reads, never DSP work.
///
/// # Errors
/// Returns invalid UID or native UID-translation errors. A selected device that
/// disappears during property observation is omitted.
pub fn selected_devices<'a>(
    selectors: impl IntoIterator<Item = &'a DeviceSelector>,
) -> Result<Vec<AudioDeviceDescriptor>, HostAudioBackendError> {
    mixrelay_audio_io::discovery::selected_inventory(&coreaudio::CoreAudioReader, selectors)
}

/// Reads visible, alive physical and virtual devices from `CoreAudio`.
///
/// Returns owned device UIDs, names, input/output channel counts and current
/// default flags. Resolve graph selectors using [`mixrelay_audio_io::DeviceSelector::resolve`]
/// against this same returned inventory so default aliases share persistent
/// device identity. No native resource is retained after the call.
///
/// Call on a control thread: discovery performs synchronous OS reads and bounded
/// allocations, and is not suitable for an audio callback. A device disappearing
/// while its properties are read is omitted, matching desktop discovery.
///
/// # Errors
/// Returns a backend error if the system device list cannot be read or its
/// returned size is invalid. Individual unavailable device properties do not
/// cause another device or a fabricated channel count to be substituted.
pub fn devices() -> Result<Vec<AudioDeviceDescriptor>, HostAudioBackendError> {
    mixrelay_audio_io::discovery::inventory(&coreaudio::CoreAudioReader)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
