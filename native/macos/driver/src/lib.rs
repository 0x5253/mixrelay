//! Independent macOS virtual-device inventory and native format control.
//!
//! The HAL owns both PCM ring writers and its existing native clock. This crate
//! only publishes bounded device inventory over XPC and observes native ACKs on
//! control; it has no graph, DSP, recording, or plugin lifetime dependency.
#![cfg(target_os = "macos")]

mod device;
mod device_observation;
mod dispatch;
mod endpoint_registry;
mod formats;
mod peer_ledger;
mod settle;
mod settlement;
mod xpc;

use endpoint_registry::{CallbackEndpoint, EndpointRegistry, EndpointSpec, OwnedInventory};
use mixrelay_audio_io::{
    NativeStreamFormat, VirtualEndpointCapabilities, VirtualEndpointDeviceSettings,
    VirtualEndpointId, VirtualEndpointSettings, VirtualEndpointSpec,
};
use mixrelay_driver_core::{DriverBackend, DriverDeviceConfig, DriverError, DriverState};
use mixrelay_macos_transport::{
    control_inventory::ProducerPolicy,
    endpoint_identity::{endpoint_uid, parse_endpoint_id},
    endpoint_transport::{EndpointClockSnapshot, mr_endpoint_transport_clock_snapshot},
};
use parking_lot::Mutex;
use settle::Progress;
use std::sync::Arc;

struct Service {
    registry: EndpointRegistry,
    producer_policy: ProducerPolicy,
    pacing: Arc<dyn settle::Pacing>,
    devices: Arc<dyn device_observation::DeviceProperties>,
    installation: Arc<dyn device_observation::HalInstallation>,
    published: Mutex<Option<OwnedInventory>>,
    generation: Mutex<u64>,
    peers: Mutex<xpc::Peers>,
    dispatch: Mutex<Option<dispatch::Dispatch>>,
    configuration: Mutex<Vec<DriverDeviceConfig>>,
    transaction: Mutex<()>,
    latencies: Mutex<
        std::collections::BTreeMap<
            [u8; 16],
            mixrelay_macos_transport::control_inventory::InputLatency,
        >,
    >,
}

impl Service {
    fn unpublished(
        producer_policy: ProducerPolicy,
        pacing: impl settle::Pacing + 'static,
        devices: impl device_observation::DeviceProperties + 'static,
        installation: impl device_observation::HalInstallation + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            registry: EndpointRegistry::new(),
            producer_policy,
            pacing: Arc::new(pacing),
            devices: Arc::new(devices),
            installation: Arc::new(installation),
            published: Mutex::new(None),
            generation: Mutex::new(0),
            peers: Mutex::new(xpc::Peers::default()),
            dispatch: Mutex::new(None),
            configuration: Mutex::new(Vec::new()),
            transaction: Mutex::new(()),
            latencies: Mutex::default(),
        })
    }

    fn publish(&self, mut snapshot: OwnedInventory) -> Result<u64, DriverError> {
        let mut generation = self.generation.lock();
        *generation = generation
            .checked_add(1)
            .ok_or_else(|| DriverError::Backend("inventory generation exhausted".into()))?;
        snapshot.set_generation(*generation);
        *self.published.lock() = Some(snapshot);
        self.peers.lock().send_inventory(self);
        Ok(*generation)
    }
}

/// One control-plane adapter in the independently launched Driver service.
/// Dropping an adapter never removes devices; only an explicit desired-inventory
/// replacement can do so. The last owner cancels its XPC resources; queued
/// callbacks hold weak ownership and cannot access a retired service.
pub struct MacDriver {
    service: Arc<Service>,
}

impl MacDriver {
    /// Starts the dedicated `io.mixrelay.audio.driver` XPC inventory owner.
    /// No empty inventory is sent before the manager loads persisted settings.
    /// # Errors
    /// Returns listener/timer errors. An installed HAL may connect after startup.
    pub fn open(producer_policy: ProducerPolicy) -> Result<Self, DriverError> {
        let service = Service::unpublished(
            producer_policy,
            mixrelay_time::StdClock::new(),
            device::CoreAudio,
            device::InstalledBundle,
        );
        let dispatch = dispatch::Dispatch::start(&service)
            .map_err(|error| DriverError::Unavailable(error.to_string()))?;
        *service.dispatch.lock() = Some(dispatch);
        Ok(Self { service })
    }

    fn bootstrap(&mut self) -> Result<(), DriverError> {
        let observed = self.service.devices.inventory()?;
        if observed.is_empty() {
            // A genuinely empty HAL remains unpublished until the manager's
            // default or explicit stored inventory is applied.
            return Ok(());
        }
        let service = &self.service;
        let _transaction = service.transaction.lock();
        let specs = observed
            .iter()
            .map(native_spec)
            .collect::<Result<Vec<_>, _>>()?;
        let prepared = service.registry.prepare_inventory_change(&specs, None)?;
        let pins = prepared.callback_pins();
        let generation = service.publish(prepared.snapshot()?)?;
        if let Err(failure) = settle::step(service.pacing.as_ref(), || {
            inventory_progress(service, generation, &pins, &specs, &specs)
        }) {
            // Same actual IDs/name/geometry only: bootstrap never removes a
            // device or writes its format/rate. Preserve that HAL configuration
            // even if attaching this service's storage cannot be acknowledged.
            service.published.lock().take();
            return Err(failure.into());
        }
        prepared.commit()?;
        *service.configuration.lock() = observed;
        service.registry.collect();
        Ok(())
    }

    fn replace_inventory(&mut self, desired: &[DriverDeviceConfig]) -> Result<(), DriverError> {
        self.replace_inventory_change(desired, None)
    }

    fn replace_inventory_change(
        &mut self,
        desired: &[DriverDeviceConfig],
        replacement: Option<[u8; 16]>,
    ) -> Result<(), DriverError> {
        let service = &self.service;
        let _transaction = service.transaction.lock();
        let specs = desired
            .iter()
            .map(native_spec)
            .collect::<Result<Vec<_>, _>>()?;
        let previous = service.configuration.lock().clone();
        let old_specs = previous
            .iter()
            .map(native_spec)
            .collect::<Result<Vec<_>, _>>()?;
        // Only manager transactions change rate or format; a HAL-requested replacement
        // must not force the cached intent over a DAW's choice the HAL now holds.
        let origin = if replacement.is_some() {
            formats::Origin::Replacement
        } else {
            formats::Origin::Manager
        };
        let wanted = specs
            .iter()
            .map(|spec| spec.id)
            .zip(desired)
            .collect::<Vec<_>>();
        let old_formats: Vec<_> = old_specs
            .iter()
            .filter(|_| origin == formats::Origin::Manager)
            .map(|spec| formats::Previous {
                id: spec.id,
                formats: service.devices.formats(spec.id),
                rate: service.devices.rate(spec.id),
            })
            .collect();
        let prepared = service
            .registry
            .prepare_inventory_change(&specs, replacement)?;
        let pins = prepared.callback_pins();
        let generation = service.publish(prepared.snapshot()?)?;
        settle::transact(
            || {
                settle::step(service.pacing.as_ref(), || {
                    inventory_progress(service, generation, &pins, &old_specs, &specs)
                })?;
                formats::apply(&Hal(service), service.pacing.as_ref(), origin, &wanted)
            },
            // Rollback uses a fresh publication generation, so HAL fencing never
            // mistakes the old active mappings for a stale provisional message.
            || service.publish(service.registry.snapshot()?),
            |generation| {
                let old_pins: Vec<_> = old_specs
                    .iter()
                    .filter_map(|spec| service.registry.callback(spec.id))
                    .collect();
                settle::step(service.pacing.as_ref(), || {
                    inventory_progress(service, generation, &old_pins, &specs, &old_specs)
                })?;
                formats::restore(&Hal(service), service.pacing.as_ref(), origin, &old_formats)
            },
        )?;
        prepared.commit()?;
        *service.configuration.lock() = desired.to_vec();
        service.registry.collect();
        Ok(())
    }
}

impl DriverBackend for MacDriver {
    fn settings(&self) -> Result<VirtualEndpointSettings, DriverError> {
        if self.state() != DriverState::Ready {
            return Err(DriverError::Unavailable(
                "MixRelay HAL is not connected".into(),
            ));
        }
        let mut settings = VirtualEndpointSettings::configurable(&capabilities());
        settings.devices = self
            .service
            .configuration
            .lock()
            .iter()
            .map(|config| {
                let id = identity(&config.spec.id)?;
                let rate = self.service.devices.rate(id);
                let supported = mixrelay_audio_io::format_contract::supported_rates(|rate| {
                    self.service.devices.supports(id, rate)
                });
                Ok(VirtualEndpointDeviceSettings {
                    spec: observed_spec(&config.spec, rate, &supported),
                    native_device_ids: if self.service.devices.present(id) {
                        vec![endpoint_uid(id)]
                    } else {
                        Vec::new()
                    },
                    observed_sample_rate: rate,
                    can_set_sample_rate: rate.is_some() && !supported.is_empty(),
                    supported_sample_rates: supported,
                    native_formats: self.service.devices.formats(id),
                })
            })
            .collect::<Result<Vec<_>, DriverError>>()?;
        Ok(settings)
    }

    fn publish_latency(
        &mut self,
        publication: &mixrelay_driver_core::RuntimeLatency,
    ) -> Result<(), DriverError> {
        let endpoint_id = identity(&publication.endpoint_id)?;
        if !self
            .service
            .configuration
            .lock()
            .iter()
            .any(|config| config.spec.id == publication.endpoint_id)
        {
            return if publication.input_latency_frames.is_none() {
                // A cleared value must not survive to republish on reactivation.
                self.service.latencies.lock().remove(&endpoint_id);
                Ok(())
            } else {
                Err(DriverError::NotFound(publication.endpoint_id.clone()))
            };
        }
        self.service.latencies.lock().insert(
            endpoint_id,
            mixrelay_macos_transport::control_inventory::InputLatency {
                endpoint_id,
                rate: publication.rate,
                engine_pid: publication.engine_lease.process_id,
                lease: publication.engine_lease.generation,
                revision: publication.applied_graph_revision,
                frames: publication.input_latency_frames,
            },
        );
        self.service.peers.lock().send_inventory(&self.service);
        Ok(())
    }

    fn replace(&mut self, desired: &[DriverDeviceConfig]) -> Result<(), DriverError> {
        if self.state() != DriverState::Ready {
            return Err(DriverError::Unavailable(
                "MixRelay HAL is not connected".into(),
            ));
        }
        let needs_bootstrap = self.service.published.lock().is_none();
        if needs_bootstrap {
            self.bootstrap()?;
        }
        self.replace_inventory(desired)
    }

    fn state(&self) -> DriverState {
        if !self.service.installation.installed() {
            DriverState::NotInstalled
        } else if self.service.peers.lock().driver_connected() {
            DriverState::Ready
        } else {
            DriverState::InstalledNotLoaded
        }
    }
}

fn capabilities() -> VirtualEndpointCapabilities {
    VirtualEndpointCapabilities {
        max_render_channels: 64,
        max_capture_channels: 64,
        supported_sample_rates: mixrelay_audio_io::formats::SAMPLE_RATES.to_vec(),
        supports_duplex: true,
        supports_dynamic_creation: true,
    }
}

fn observed_spec(
    configured: &VirtualEndpointSpec,
    rate: Option<u32>,
    supported: &[u32],
) -> VirtualEndpointSpec {
    let mut spec = configured.clone();
    if let Some(rate) = rate {
        spec.default_sample_rate = rate;
    }
    spec.supported_sample_rates = supported.to_vec();
    spec
}

fn identity(id: &VirtualEndpointId) -> Result<[u8; 16], DriverError> {
    parse_endpoint_id(id.as_str()).ok_or_else(|| {
        DriverError::Invalid(format!(
            "device identity must be a canonical lowercase UUID: {}",
            id.as_str()
        ))
    })
}

fn native_spec(config: &DriverDeviceConfig) -> Result<EndpointSpec, DriverError> {
    config
        .spec
        .validate(&capabilities())
        .map_err(|error| DriverError::Invalid(error.to_string()))?;
    for (format, stream) in [
        (config.render_format, config.spec.render),
        (config.capture_format, config.spec.capture),
    ] {
        if let Some(format) = format
            && (stream.is_none_or(|stream| stream.channels != format.channels)
                || format.sample_rate != config.spec.default_sample_rate)
        {
            return Err(DriverError::Invalid(
                "directional format must match device channels and shared nominal rate".into(),
            ));
        }
    }
    Ok(EndpointSpec {
        id: identity(&config.spec.id)?,
        display_name: config.spec.display_name.clone(),
        sample_rate: config.spec.default_sample_rate,
        // Endpoint rings carry only lifecycle metadata; client PCM queues use the
        // same playback-sized bound. Capture retention lives in the engine's sources.
        capacity_frames: mixrelay_macos_transport::client_pcm::QUEUE_FRAMES,
        render_channels: config
            .spec
            .render
            .map_or(0, |stream| u32::from(stream.channels)),
        capture_channels: config
            .spec
            .capture
            .map_or(0, |stream| u32::from(stream.channels)),
    })
}

fn inventory_progress(
    service: &Service,
    generation: u64,
    pins: &[CallbackEndpoint<'_>],
    before: &[EndpointSpec],
    after: &[EndpointSpec],
) -> Progress {
    let peers = service.peers.lock();
    if peers.incompatible() {
        return Progress::Incompatible;
    }
    let settlement = peers.settlement(generation);
    drop(peers);
    match settlement {
        Some(xpc::Settlement::Applied) => {
            Progress::applied(inventory_applied(service, pins, before, after))
        }
        Some(xpc::Settlement::Rejected) => Progress::Rejected,
        None => Progress::Performing,
    }
}

fn inventory_applied(
    service: &Service,
    pins: &[CallbackEndpoint<'_>],
    before: &[EndpointSpec],
    after: &[EndpointSpec],
) -> bool {
    pins.iter().all(|pin| {
        let mut clock = EndpointClockSnapshot::default();
        // A fresh mapping becomes ready only when the HAL has actually created
        // or performed its attachment under Device::freeze, not upon XPC receipt.
        (unsafe { mr_endpoint_transport_clock_snapshot(pin.transport(), &raw mut clock) })
            && service.devices.present(pin.spec().id)
    }) && before
        .iter()
        .filter(|spec| !after.iter().any(|next| next.id == spec.id))
        .all(|spec| !service.devices.present(spec.id))
}

/// The installed HAL as seen by a configuration transaction.
struct Hal<'a>(&'a Service);

impl formats::Native for Hal<'_> {
    fn rate(&self, id: endpoint_registry::EndpointId) -> Option<u32> {
        self.0.devices.rate(id)
    }
    fn set_rate(&self, id: endpoint_registry::EndpointId, rate: u32) -> bool {
        self.0.devices.set_rate(id, rate)
    }
    fn format(
        &self,
        id: endpoint_registry::EndpointId,
        render: bool,
    ) -> Option<NativeStreamFormat> {
        let formats = self.0.devices.formats(id);
        if render {
            formats.render.current
        } else {
            formats.capture.current
        }
    }
    fn set_format(
        &self,
        id: endpoint_registry::EndpointId,
        render: bool,
        format: NativeStreamFormat,
    ) -> bool {
        self.0.devices.set_format(id, render, format)
    }
    fn since(&self) -> Option<settlement::Since> {
        self.0.peers.lock().since()
    }
    fn settled(
        &self,
        id: endpoint_registry::EndpointId,
        since: settlement::Since,
        wanted: settlement::Wanted,
    ) -> Option<settlement::Configured> {
        self.0.peers.lock().settled(id, since, wanted)
    }
    fn incompatible(&self) -> bool {
        self.0.peers.lock().incompatible()
    }
}

#[cfg(test)]
mod tests;
