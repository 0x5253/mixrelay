//! The engine's graph resource port: device inventory, reconciliation and format replacement.
use super::{
    Aggregate, MacDevices, Publication, application, backend_error, native::Native, spec::specs,
};
use mixrelay_audio_graph::{GraphDocument, RenderFormat};
use mixrelay_audio_io::HostAudioBackendError;
use mixrelay_engine_core::{GraphResourceBackend, PreparedGraphFormat};
use std::sync::atomic::Ordering;

impl GraphResourceBackend for MacDevices {
    fn execution_released(&self) -> bool {
        Native::active_owners() == 0 && Aggregate::active_owners() == 0
    }
    fn device_inventory(
        &self,
        selectors: &[&mixrelay_audio_io::DeviceSelector],
    ) -> Result<Vec<mixrelay_audio_io::AudioDeviceDescriptor>, HostAudioBackendError> {
        self.require_preparation()?;
        self.host.inventory(selectors)
    }

    fn resolve_application_capture(
        &self,
        request: &mixrelay_audio_io::ApplicationCaptureRequest,
    ) -> Result<mixrelay_audio_graph::composition::SharedSource, HostAudioBackendError> {
        application::resolve(request)
    }

    fn prepare_format_change(
        &self,
        document: &GraphDocument,
        playback: mixrelay_engine_core::PlaybackBindings<'_>,
        current: RenderFormat,
        requested: RenderFormat,
    ) -> Result<Box<dyn PreparedGraphFormat>, HostAudioBackendError> {
        application::validate_playback(&playback)?;
        self.prepare_device_format(document, current, requested)
    }
    fn reconcile(
        &self,
        document: &GraphDocument,
        playback: mixrelay_engine_core::PlaybackBindings<'_>,
    ) -> Result<(), HostAudioBackendError> {
        application::validate_playback(&playback)?;
        self.require_preparation()?;
        let desired = specs(document);
        // Adding a virtual endpoint does not transfer hardware's clock authority
        // to that endpoint. Cross-clock transport is an explicit bridge boundary.
        let follow_hardware = !desired.is_empty();
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| backend_error("CoreAudio device cache poisoned"))?;
        if desired.len() == cache.len()
            && desired.iter().all(|(id, spec)| {
                cache
                    .get(id)
                    .is_some_and(|(old, device)| old == spec && device.strong_count() != 0)
            })
        {
            self.prepared_hardware_clock
                .store(follow_hardware, Ordering::Release);
            return Ok(());
        }
        self.handoff.reclaim();
        if !self.handoff.idle() {
            return Err(backend_error("CoreAudio device update is already pending"));
        }
        self.prepare_devices(&mut cache, &desired, Publication::Graph)?;
        self.prepared_hardware_clock
            .store(follow_hardware, Ordering::Release);
        Ok(())
    }
}
