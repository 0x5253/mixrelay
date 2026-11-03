//! Nominal rate and directional format requests within a Driver configuration transaction.
use crate::{
    endpoint_registry::EndpointId,
    settle::{self, Failure, Pacing, Progress},
    settlement::{Configured, Since, Wanted},
};
use mixrelay_audio_io::{NativeDeviceFormats, NativeSampleFormat, NativeStreamFormat};
use mixrelay_driver_core::{DriverDeviceConfig, DriverError};

/// Who started a configuration transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// The manager applies a user or startup intent and converges rate and formats on it.
    Manager,
    /// The HAL asked for a fresh mapping. Only the mapping changes: the service's cached
    /// intent may predate a DAW's rate or format choice that the HAL now holds.
    Replacement,
}

/// A device's rate and formats before a transaction, restored if it fails.
pub(crate) struct Previous {
    pub(crate) id: EndpointId,
    pub(crate) formats: NativeDeviceFormats,
    pub(crate) rate: Option<u32>,
}

/// The HAL device properties and request reports a transaction observes.
pub(crate) trait Native {
    fn rate(&self, id: EndpointId) -> Option<u32>;
    fn set_rate(&self, id: EndpointId, rate: u32) -> bool;
    fn format(&self, id: EndpointId, render: bool) -> Option<NativeStreamFormat>;
    fn set_format(&self, id: EndpointId, render: bool, format: NativeStreamFormat) -> bool;
    /// The settlement bound for a request issued now; `None` until the HAL connection
    /// has replayed what it retained.
    fn since(&self) -> Option<Since>;
    /// The first settlement after `since` of a request for exactly `wanted`.
    fn settled(&self, id: EndpointId, since: Since, wanted: Wanted) -> Option<Configured>;
    /// Whether the HAL reports with another protocol version.
    fn incompatible(&self) -> bool;
}

/// Requests the rate and formats `desired` still lacks. The HAL readback, not the
/// service's cached intent, decides what is missing: a Perform reported pending may
/// have settled since, and a DAW may have changed the device.
pub(crate) fn apply(
    native: &impl Native,
    pacing: &(impl Pacing + ?Sized),
    origin: Origin,
    desired: &[(EndpointId, &DriverDeviceConfig)],
) -> Result<(), Failure> {
    if origin == Origin::Replacement {
        return Ok(());
    }
    for (id, config) in desired {
        converge(
            native,
            pacing,
            *id,
            Some(config.spec.default_sample_rate),
            [config.capture_format, config.render_format],
        )?;
    }
    Ok(())
}

/// Restores what `apply` may have changed.
pub(crate) fn restore(
    native: &impl Native,
    pacing: &(impl Pacing + ?Sized),
    origin: Origin,
    previous: &[Previous],
) -> Result<(), Failure> {
    if origin == Origin::Replacement {
        return Ok(());
    }
    for Previous { id, formats, rate } in previous {
        converge(
            native,
            pacing,
            *id,
            *rate,
            [formats.capture.current, formats.render.current],
        )?;
    }
    Ok(())
}

/// Brings one device to `rate`, then each wanted `[capture, render]` format.
fn converge(
    native: &impl Native,
    pacing: &(impl Pacing + ?Sized),
    id: EndpointId,
    rate: Option<u32>,
    formats: [Option<NativeStreamFormat>; 2],
) -> Result<(), Failure> {
    if let Some(rate) = missing(rate, native.rate(id)) {
        request(
            native,
            pacing,
            (id, Wanted::Rate(rate)),
            || native.set_rate(id, rate),
            || native.rate(id) == Some(rate),
        )?;
    }
    for (render, wanted) in [(false, formats[0]), (true, formats[1])] {
        if let Some(format) = missing(wanted, native.format(id, render)) {
            let wanted = Wanted::Format {
                render,
                rate: format.sample_rate,
                format: code(format.sample_format),
            };
            request(
                native,
                pacing,
                (id, wanted),
                || native.set_format(id, render, format),
                || native.format(id, render) == Some(format),
            )?;
        }
    }
    Ok(())
}

/// The wanted value when HAL readback does not already report it.
fn missing<T: Copy + PartialEq>(wanted: Option<T>, observed: Option<T>) -> Option<T> {
    wanted.filter(|wanted| observed != Some(*wanted))
}

/// The HAL's `SampleFormat` code for a native sample format.
fn code(format: NativeSampleFormat) -> u32 {
    match format {
        NativeSampleFormat::Float32 => 1,
        NativeSampleFormat::Signed16 => 2,
        NativeSampleFormat::Signed24 => 3,
        NativeSampleFormat::Signed32 => 4,
    }
}

/// Issues one request and waits until the HAL applies it or reports its rejection.
fn request(
    native: &impl Native,
    pacing: &(impl Pacing + ?Sized),
    (id, wanted): (EndpointId, Wanted),
    issue: impl FnOnce() -> bool,
    applied: impl Fn() -> bool,
) -> Result<(), Failure> {
    let since = replayed(native, pacing);
    if !issue() {
        return Err(Failure::Rejected(DriverError::Backend(
            match wanted {
                Wanted::Rate(_) => "native nominal rate rejected",
                Wanted::Format { .. } => "native directional ASBD rejected",
            }
            .into(),
        )));
    }
    settle::step(pacing, || {
        if native.incompatible() {
            Progress::Incompatible
        } else if applied() {
            Progress::Applied
        } else if since
            .and_then(|since| native.settled(id, since, wanted))
            .is_some_and(|settled| !settled.applied)
        {
            Progress::Rejected
        } else {
            Progress::Performing
        }
    })
}

/// The settlement bound once the HAL connection has replayed its retained settlements.
/// A fresh connection replays within one control tick; past `REPLAY_TIMEOUT` the request
/// is attributed by readback alone, and a rejection then shows up as a timeout.
fn replayed(native: &impl Native, pacing: &(impl Pacing + ?Sized)) -> Option<Since> {
    let deadline = pacing.now().saturating_add(REPLAY_TIMEOUT);
    loop {
        if let Some(since) = native.since() {
            return Some(since);
        }
        if pacing.now() >= deadline {
            return None;
        }
        pacing.pause();
    }
}

/// Ten HAL control ticks of 100 ms.
const REPLAY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

#[cfg(test)]
mod tests;
