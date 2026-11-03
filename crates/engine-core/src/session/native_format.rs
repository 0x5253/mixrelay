//! Owns native format callbacks; session policy controls when they may run.

use mixrelay_audio_graph::RenderFormat;
use std::sync::Arc;

type DeviceRateSetter = fn(u32) -> Result<(), String>;
type DeviceRateReader = Arc<dyn Fn() -> Option<u32> + Send + Sync>;
type DeviceFormatSetter = Arc<dyn Fn(RenderFormat) -> Result<(), String> + Send + Sync>;
type DeviceFormatReader = Arc<dyn Fn() -> Option<RenderFormat> + Send + Sync>;

#[derive(Default)]
pub(super) struct NativeFormat {
    set_rate: Option<DeviceRateSetter>,
    set_format: Option<DeviceFormatSetter>,
    read_format: Option<DeviceFormatReader>,
    graph_control: bool,
    read_rate: Option<DeviceRateReader>,
}

impl NativeFormat {
    pub(super) fn set_rate_control(&mut self, setter: DeviceRateSetter) {
        self.set_rate = Some(setter);
    }
    pub(super) fn set_pair_control(&mut self, setter: DeviceFormatSetter) {
        self.set_format = Some(setter);
    }
    pub(super) fn follow_pair(&mut self, reader: DeviceFormatReader) {
        self.read_format = Some(reader);
    }
    pub(super) fn allow_graph_control(&mut self) {
        self.graph_control = true;
    }
    pub(super) fn follow_rate(&mut self, reader: DeviceRateReader) {
        self.read_rate = Some(reader);
    }
    pub(super) fn has_rate_control(&self) -> bool {
        self.set_rate.is_some()
    }
    pub(super) fn has_pair_control(&self) -> bool {
        self.set_format.is_some()
    }
    pub(super) fn follows_rate(&self) -> bool {
        self.read_rate.is_some()
    }
    pub(super) fn follows_pair(&self) -> bool {
        self.read_format.is_some()
    }
    pub(super) fn can_reconfigure(&self) -> bool {
        self.has_rate_control() || self.has_pair_control() || self.graph_control
    }
    pub(super) fn owns_format(&self) -> bool {
        self.can_reconfigure() || self.follows_rate() || self.follows_pair()
    }
    pub(super) fn read_pair(&self) -> Option<RenderFormat> {
        self.read_format.as_ref().and_then(|reader| reader())
    }
    pub(super) fn read_rate(&self) -> Option<u32> {
        self.read_rate.as_ref().and_then(|reader| reader())
    }

    pub(super) fn write(
        &self,
        requested: RenderFormat,
        observe: impl FnOnce(Option<RenderFormat>),
    ) -> Result<(), String> {
        if let Some(setter) = &self.set_format {
            setter(requested)?;
            let actual = self.read_pair();
            observe(actual);
            if actual != Some(requested) {
                return Err("native format did not confirm the requested pair".into());
            }
            Ok(())
        } else {
            self.set_rate
                .map_or(Ok(()), |setter| setter(requested.sample_rate))
        }
    }

    pub(super) fn rollback_format(&self, fallback: RenderFormat) -> Result<RenderFormat, String> {
        if self.has_pair_control() {
            self.read_pair()
                .ok_or_else(|| "native format was unavailable before negotiation".into())
        } else {
            Ok(fallback)
        }
    }
}
