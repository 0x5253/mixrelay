use super::{
    backend,
    loopback::{Loopback, MAX_FRAMES},
    native_name, native_names,
};
use mixrelay_audio_io::{
    NativeDeviceFormats, NativeSampleFormat, NativeStreamFormat, NativeStreamFormats,
    VirtualEndpointDeviceSettings,
};
use mixrelay_driver_control::{DriverDeviceConfig, DriverError};
use mixrelay_pipewire::{
    DriverFormat, GraphDriver, NativeStream, StreamOwners, StreamProperties, StreamRole,
};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::plan::requested_formats;

pub(super) fn validate(config: &DriverDeviceConfig) -> Result<(), DriverError> {
    crate::plan::DevicePlan::new(config)
        .map(|_| ())
        .map_err(|error| DriverError::Invalid(error.to_string()))
}

pub(super) struct NativeDevices {
    owners: StreamOwners,
    clock: std::sync::Arc<dyn crate::admission::ControlTime>,
}
impl NativeDevices {
    pub(super) fn new(
        owners: StreamOwners,
        clock: std::sync::Arc<dyn crate::admission::ControlTime>,
    ) -> Self {
        Self { owners, clock }
    }
}
impl crate::reconcile::DeviceOwners for NativeDevices {
    type Error = DriverError;
    type Prepared = DevicePreparation;
    type Owner = DeviceRuntime;
    fn prepare(&self, plan: &crate::plan::DevicePlan) -> Result<DevicePreparation, DriverError> {
        DevicePreparation::with_clock(&plan.config, self.clock.clone())
    }
    fn open(&self, prepared: DevicePreparation) -> Result<DeviceRuntime, DriverError> {
        DeviceRuntime::open(prepared, self.clock.as_ref(), &self.owners)
    }
}
impl crate::reconcile::DeviceOwner for DeviceRuntime {
    type Error = DriverError;
    fn config(&self) -> &DriverDeviceConfig {
        &self.config
    }
    fn ready(&self) -> bool {
        self.ready()
    }
    fn failed(&self) -> bool {
        self.failed()
    }
    fn settings(&self) -> Result<VirtualEndpointDeviceSettings, DriverError> {
        self.settings()
    }
}

pub(super) struct DevicePreparation {
    pub(super) config: DriverDeviceConfig,
    name: String,
    render_format: Option<NativeStreamFormat>,
    capture_format: Option<NativeStreamFormat>,
    process: Loopback,
}

impl DevicePreparation {
    #[cfg(test)]
    pub(super) fn new(config: &DriverDeviceConfig) -> Result<Self, DriverError> {
        Self::with_clock(
            config,
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        )
    }

    fn with_clock(
        config: &DriverDeviceConfig,
        clock: std::sync::Arc<dyn crate::admission::ControlTime>,
    ) -> Result<Self, DriverError> {
        validate(config)?;
        let [render_format, capture_format] = requested_formats(config);
        let render_channels = render_format.map_or(0, |format| usize::from(format.channels));
        let capture_channels = capture_format.map_or(0, |format| usize::from(format.channels));
        let mut input = mixrelay_audio_io::pcm_buffer::zeroed(render_channels * MAX_FRAMES)
            .map_err(backend)?
            .into_boxed_slice();
        let mut output =
            mixrelay_audio_io::pcm_buffer::zeroed(if render_channels == capture_channels {
                0
            } else {
                capture_channels * MAX_FRAMES
            })
            .map_err(backend)?
            .into_boxed_slice();
        // calloc/mmap can leave zeroed pages uncommitted. Prepare the entire
        // callback storage on control before publishing the native processor.
        mixrelay_pcm_ring::prefault_pcm(&mut input);
        mixrelay_pcm_ring::prefault_pcm(&mut output);
        Ok(Self {
            config: config.clone(),
            name: native_name(&config.spec.id),
            render_format,
            capture_format,
            process: Loopback {
                clock,
                render: None,
                capture: None,
                sample_rate: config.spec.default_sample_rate,
                failure: None,
                render_channels,
                capture_channels,
                input,
                output,
                mapper: mixrelay_audio_io::frame_budget::FrameBudgetMapper::new(
                    config.spec.default_sample_rate,
                )
                .map_err(backend)?,
            },
        })
    }
}

pub(super) struct DeviceRuntime {
    pub(super) config: DriverDeviceConfig,
    render: Option<StreamProperties>,
    capture: Option<StreamProperties>,
    driver: GraphDriver<Loopback>,
    reported_process_failure: AtomicBool,
}

impl DeviceRuntime {
    pub(super) fn open(
        mut preparation: DevicePreparation,
        clock: &dyn crate::admission::ControlTime,
        owners: &StreamOwners,
    ) -> Result<Self, DriverError> {
        let description = &preparation.config.spec.display_name;
        if let Some(format) = preparation.render_format {
            preparation.process.render = Some(
                NativeStream::open_pcm_format(
                    owners,
                    &format!("{}.render", preparation.name),
                    description,
                    StreamRole::VirtualRender,
                    format,
                    128,
                )
                .map_err(backend)?,
            );
        }
        if let Some(format) = preparation.capture_format {
            preparation.process.capture = Some(
                NativeStream::open_pcm_format(
                    owners,
                    &format!("{}.capture", preparation.name),
                    description,
                    StreamRole::VirtualCapture,
                    format,
                    128,
                )
                .map_err(backend)?,
            );
        }
        let render = preparation
            .process
            .render
            .as_ref()
            .map(NativeStream::properties);
        let capture = preparation
            .process
            .capture
            .as_ref()
            .map(NativeStream::properties);
        let format =
            DriverFormat::new(preparation.config.spec.default_sample_rate, 128).map_err(backend)?;
        let driver = GraphDriver::open(preparation.process, format).map_err(backend)?;
        wait_for_native_tuple(
            clock,
            &driver,
            &preparation.config.spec.id,
            [
                (render.as_ref(), preparation.render_format),
                (capture.as_ref(), preparation.capture_format),
            ],
        )?;
        Ok(Self {
            config: preparation.config,
            render,
            capture,
            driver,
            reported_process_failure: AtomicBool::new(false),
        })
    }

    pub(super) fn ready(&self) -> bool {
        self.driver.process_failure().is_none()
            && self.driver.ready()
            && self.render.iter().chain(self.capture.iter()).all(|stream| {
                stream
                    .native_formats()
                    .is_some_and(|(_, current)| current.is_some())
            })
    }

    pub(super) fn failed(&self) -> bool {
        let processing_failure = self.driver.process_failure();
        // This control-only flag deduplicates diagnostics; native owners publish the faults.
        if !self.reported_process_failure.load(Ordering::Relaxed) {
            if let Some(cause) = processing_failure {
                tracing::warn!(
                    device_id = self.config.spec.id.as_str(),
                    ?cause,
                    "PipeWire device processing failed"
                );
                self.reported_process_failure.store(true, Ordering::Relaxed);
            } else {
                for (direction, stream) in [("render", &self.render), ("capture", &self.capture)] {
                    if let Some(native_code) = stream
                        .as_ref()
                        .and_then(StreamProperties::buffer_return_error)
                    {
                        tracing::warn!(
                            device_id = self.config.spec.id.as_str(),
                            direction,
                            native_code,
                            "PipeWire buffer return failed"
                        );
                        self.reported_process_failure.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
        }
        processing_failure.is_some()
            || self.driver.failed()
            || self
                .render
                .iter()
                .chain(self.capture.iter())
                .any(StreamProperties::failed)
    }

    pub(super) fn settings(&self) -> Result<VirtualEndpointDeviceSettings, DriverError> {
        let render = formats(self.render.as_ref())?;
        let capture = formats(self.capture.as_ref())?;
        let rate = render
            .current
            .or(capture.current)
            .map(|format| format.sample_rate);
        Ok(VirtualEndpointDeviceSettings {
            spec: self.config.spec.clone(),
            native_device_ids: native_names(&self.config.spec),
            observed_sample_rate: rate,
            supported_sample_rates: rate.into_iter().collect(),
            can_set_sample_rate: rate.is_some() && self.ready(),
            native_formats: NativeDeviceFormats { render, capture },
        })
    }
}

fn wait_for_native_tuple(
    clock: &dyn crate::admission::ControlTime,
    driver: &GraphDriver<Loopback>,
    id: &mixrelay_audio_io::VirtualEndpointId,
    directions: [(Option<&StreamProperties>, Option<NativeStreamFormat>); 2],
) -> Result<(), DriverError> {
    use crate::admission::{DirectionEvidence, Evidence};
    let mut processing_failure = None;
    let result = crate::admission::wait(directions.map(|(_, requested)| requested), clock, || {
        let observed = directions.map(|(properties, _)| {
            properties.map_or(DirectionEvidence::Absent, |properties| {
                properties
                    .native_formats()
                    .map_or(DirectionEvidence::Unobserved, |(_, current)| {
                        DirectionEvidence::Current(current)
                    })
            })
        });
        let ready = driver.ready();
        processing_failure = driver.process_failure();
        Evidence {
            directions: observed,
            ready,
            processing_failed: processing_failure.is_some(),
            disconnected: driver.failed(),
        }
    });
    if let (Err(crate::admission::AdmissionError::ProcessingFailed), Some(cause)) =
        (&result, processing_failure)
    {
        tracing::error!(
            device_id = id.as_str(),
            ?cause,
            "PipeWire device processing failed before admission"
        );
        return Err(DriverError::Backend(format!(
            "native device processing failed before admission: {cause:?}"
        )));
    }
    result.map_err(backend)
}

fn formats(properties: Option<&StreamProperties>) -> Result<NativeStreamFormats, DriverError> {
    let Some(properties) = properties else {
        return Ok(NativeStreamFormats::default());
    };
    let (_, current) = properties
        .native_formats()
        .ok_or_else(|| DriverError::Unavailable("native owner retired".into()))?;
    // These are Driver-reconfiguration capabilities at this opened nominal rate,
    // not a claim that the selected acquisition advertises alternative formats.
    // Each selection opens a singleton EnumFormat and verifies actual readback.
    let supported = current
        .into_iter()
        .flat_map(|current| {
            [
                NativeSampleFormat::Float32,
                NativeSampleFormat::Signed16,
                NativeSampleFormat::Signed24,
                NativeSampleFormat::Signed32,
            ]
            .map(|sample_format| NativeStreamFormat {
                sample_format,
                ..current
            })
        })
        .collect();
    Ok(NativeStreamFormats { current, supported })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "native-lab")]
    use mixrelay_driver_control::DriverBackend;
    use mixrelay_pipewire::{DriverCycle, DriverProcess};
    #[cfg(feature = "native-lab")]
    use std::time::{Duration, Instant};

    #[test]
    fn implicit_float_formats_match_the_manager_actual_readback() {
        let implicit = crate::tests::config(4);
        let mut observed = implicit.clone();
        let [render, capture] = requested_formats(&implicit);
        observed.render_format = render;
        observed.capture_format = capture;
        assert_eq!(requested_formats(&implicit), requested_formats(&observed));
        observed.render_format.as_mut().unwrap().sample_format = NativeSampleFormat::Signed24;
        assert_ne!(requested_formats(&implicit), requested_formats(&observed));
    }

    #[test]
    #[cfg(feature = "native-lab")]
    fn native_driver_failed_stream_recovers_same_config_and_preserves_healthy_owner() {
        assert!(std::env::var_os("MIXRELAY_LAB_APPLICATION_FIXTURE").is_some());
        let mut failed = crate::tests::config(2);
        let format = NativeStreamFormat {
            sample_rate: 48_000,
            channels: 2,
            sample_format: NativeSampleFormat::Signed24,
        };
        failed.render_format = Some(format);
        failed.capture_format = Some(format);
        let healthy = crate::tests::config(4);
        let desired = [failed, healthy];
        let mut owner = crate::LinuxDriver::open().unwrap();
        owner.replace(&desired).unwrap();
        let before = owner.settings().unwrap();
        let node_ids = |runtime: &DeviceRuntime| {
            runtime
                .render
                .iter()
                .chain(runtime.capture.iter())
                .map(|properties| properties.node_id().unwrap())
                .collect::<Vec<_>>()
        };
        let failed_key = desired[0].spec.id.as_str();
        let healthy_key = desired[1].spec.id.as_str();
        let failed_nodes = node_ids(&owner.inventory.owners()[failed_key]);
        let healthy_nodes = node_ids(&owner.inventory.owners()[healthy_key]);
        let mut observed_intent = desired.clone();
        for config in &mut observed_intent {
            let [render, capture] = requested_formats(config);
            config.render_format = render;
            config.capture_format = capture;
        }
        assert!(!owner.recover(&observed_intent).unwrap());
        let command = std::process::Command::new("pw-cli")
            .args(["destroy", &failed_nodes[0].to_string()])
            .output()
            .unwrap();
        assert!(command.status.success(), "{command:?}");
        let deadline = Instant::now() + Duration::from_secs(3);
        while !owner.inventory.owners()[failed_key].failed() {
            assert!(
                Instant::now() < deadline,
                "destroyed stream did not report native failure"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!owner.inventory.owners()[healthy_key].failed());
        assert!(owner.recover(&observed_intent).unwrap());
        assert_eq!(owner.settings().unwrap(), before);
        assert_eq!(
            node_ids(&owner.inventory.owners()[healthy_key]),
            healthy_nodes
        );
        assert!(!owner.inventory.owners()[failed_key].failed());
        assert!(!owner.recover(&desired).unwrap());
        owner.replace(&[]).unwrap();
        assert!(!owner.recover(&[]).unwrap());
        assert!(owner.inventory.owners().is_empty());
    }

    #[test]
    fn preparation_reserves_output_only_for_channel_projection() {
        for (render, capture) in [
            (2, 2),
            (32, 32),
            (64, 64),
            (32, 64),
            (64, 32),
            (0, 64),
            (64, 0),
        ] {
            let mut config = crate::tests::config(render.max(capture));
            config.spec.render = (render != 0).then_some(mixrelay_audio_io::StreamSpec {
                channels: render,
                channel_layout: mixrelay_audio_io::ChannelLayout::Discrete,
            });
            config.spec.capture = (capture != 0).then_some(mixrelay_audio_io::StreamSpec {
                channels: capture,
                channel_layout: mixrelay_audio_io::ChannelLayout::Discrete,
            });
            let mut preparation = DevicePreparation::new(&config).unwrap();
            assert_eq!(
                preparation.process.input.len(),
                usize::from(render) * MAX_FRAMES
            );
            let expected_output = if render == capture {
                0
            } else {
                usize::from(capture) * MAX_FRAMES
            };
            assert_eq!(preparation.process.output.len(), expected_output);
            preparation.process.process(&DriverCycle {
                position: 0,
                monotonic_ns: 0,
                clock_id: 1,
                rate: 48_000,
                frames: 128,
            });
        }
    }
}
