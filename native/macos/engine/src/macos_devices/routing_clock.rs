//! Hardware routing clock adoption. A graph with selected devices follows its pacing
//! device's native rate; otherwise the session format stays with the runtime status.
use super::{MacDevices, backend_error};
use mixrelay_audio_graph::RenderFormat;
use mixrelay_audio_io::HostAudioBackendError;
use mixrelay_engine_core::{PreparedSessionFormat, SessionFormatBackend};
use std::sync::atomic::Ordering;

impl MacDevices {
    pub fn follows_hardware(&self) -> bool {
        self.hardware_clock.load(Ordering::Acquire)
    }

    pub fn routing_rate(&self) -> Option<u32> {
        if !self.follows_hardware() {
            return None;
        }
        self.cache
            .lock()
            .ok()?
            .values()
            .filter_map(|(_, owner)| owner.upgrade())
            .filter(|device| !device.failed.load(Ordering::Acquire))
            .min_by_key(|device| device.input)
            .map(|device| device.native_rate.load(Ordering::Acquire))
            .filter(|rate| *rate != 0)
    }
}

impl SessionFormatBackend for MacDevices {
    fn external_sample_rate(&self) -> Option<u32> {
        self.status.external_sample_rate()
    }
    fn external_format(&self, current: RenderFormat) -> Option<RenderFormat> {
        if self.follows_hardware() {
            return self.routing_rate().map(|sample_rate| RenderFormat {
                sample_rate,
                ..current
            });
        }
        self.status.external_format(current)
    }
    fn adopt_external_format(&self, requested: RenderFormat) -> Result<(), HostAudioBackendError> {
        if self.follows_hardware() {
            if self.routing_rate() != Some(requested.sample_rate) {
                return Err(backend_error("routing clock changed during preparation"));
            }
            self.status.adopt_routing_clock(requested)?;
            if self.routing_rate() != Some(requested.sample_rate) {
                return Err(backend_error("routing clock changed during suspension"));
            }
            return Ok(());
        }
        self.status.adopt_external_format(requested)
    }
    fn prepare_format(
        &self,
        requested: RenderFormat,
    ) -> Result<Box<dyn PreparedSessionFormat + '_>, HostAudioBackendError> {
        if self.follows_hardware() {
            if self.routing_rate() != Some(requested.sample_rate) {
                return Err(backend_error(
                    "hardware routing follows its device sample rate; change the selected device format, not the unrelated virtual endpoint",
                ));
            }
            return self.status.prepare_routing_buffer(requested);
        }
        self.status.prepare_format(requested)
    }
}
