//! Bounded aggregate admission after its input streams and callback layout appear.

use super::{Address, CoreAudio, TapError, TapFormat, object_ids, property, property_at};
use mixrelay_macos_transport::audio_buffer::AudioBuffer;
use std::{ptr, time::Duration};

#[cfg(test)]
mod tests;

// Keep the existing half-second readiness budget. Native create/property calls
// themselves are synchronous; this budget bounds retries, not OS IPC duration.
const ATTEMPTS: usize = 500;
const MAX_BUFFERS: usize = 64;

pub(super) trait Probe {
    fn alive(&self, aggregate: u32) -> bool;
    fn format(&self, tap: u32) -> Result<TapFormat, TapError>;
    fn streams(&self, aggregate: u32) -> Result<Vec<TapFormat>, TapError>;
    fn channels(&self, aggregate: u32) -> Option<u32>;
    fn pause(&self);
}

impl Probe for CoreAudio {
    fn alive(&self, aggregate: u32) -> bool {
        property::<u32>(aggregate, *b"livn").is_ok_and(|value| value != 0)
    }

    fn format(&self, tap: u32) -> Result<TapFormat, TapError> {
        property(tap, *b"tfmt")
    }

    fn streams(&self, aggregate: u32) -> Result<Vec<TapFormat>, TapError> {
        let ids = object_ids(aggregate, Address::input(*b"stm#"))?;
        if ids.is_empty() || ids.len() > MAX_BUFFERS || ids.contains(&0) {
            return Err(TapError::code(5));
        }
        let mut formats = Vec::new();
        formats
            .try_reserve_exact(ids.len())
            .map_err(|_| TapError::code(6))?;
        for id in ids {
            formats.push(property_at(id, Address::global(*b"sfmt"))?);
        }
        Ok(formats)
    }

    fn channels(&self, aggregate: u32) -> Option<u32> {
        configuration_channels(aggregate)
    }

    fn pause(&self) {
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub(super) fn wait(tap: u32, aggregate: u32, probe: &impl Probe) -> Result<(), TapError> {
    for attempt in 0..ATTEMPTS {
        if ready(tap, aggregate, probe) {
            return Ok(());
        }
        if attempt + 1 < ATTEMPTS {
            probe.pause();
        }
    }
    Err(TapError::code(4))
}

fn ready(tap: u32, aggregate: u32, probe: &impl Probe) -> bool {
    if !probe.alive(aggregate) {
        return false;
    }
    let Ok(format) = probe.format(tap) else {
        return false;
    };
    if !format.supported() || probe.channels(aggregate) != Some(format.channels) {
        return false;
    }
    let Ok(streams) = probe.streams(aggregate) else {
        return false;
    };
    let total = streams.iter().try_fold(0_u32, |channels, stream| {
        (stream.supported() && (stream.sample_rate - format.sample_rate).abs() < 0.5)
            .then(|| channels.checked_add(stream.channels))
            .flatten()
    });
    !streams.is_empty() && total == Some(format.channels)
}

#[repr(C)]
struct Configuration {
    count: u32,
    buffers: [AudioBuffer; MAX_BUFFERS],
}

fn configuration_channels(aggregate: u32) -> Option<u32> {
    let mut config = Configuration {
        count: 0,
        buffers: std::array::from_fn(|_| AudioBuffer {
            channels: 0,
            byte_size: 0,
            data: ptr::null_mut(),
        }),
    };
    let mut size = u32::try_from(size_of::<Configuration>()).ok()?;
    let address = Address::input(*b"slay");
    // SAFETY: local repr(C) storage has room for 64 complete AudioBuffers. Only
    // the returned, validated extent is inspected; no buffer pointer is followed.
    let status = unsafe {
        super::AudioObjectGetPropertyData(
            aggregate,
            &raw const address,
            0,
            ptr::null(),
            &raw mut size,
            ptr::from_mut(&mut config).cast(),
        )
    };
    let count = config.count as usize;
    if status != 0
        || count > MAX_BUFFERS
        || size as usize
            != std::mem::offset_of!(Configuration, buffers) + count * size_of::<AudioBuffer>()
    {
        return None;
    }
    config.buffers[..count]
        .iter()
        .try_fold(0_u32, |total, buffer| total.checked_add(buffer.channels))
}
