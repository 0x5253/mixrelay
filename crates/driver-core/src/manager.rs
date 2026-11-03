//! Driver inventory policy over native and settings-storage ports.

use std::collections::BTreeSet;

use mixrelay_audio_io::{
    ChannelLayout, NativeSampleFormat, NativeStreamFormat, StreamSpec, VirtualEndpointId,
    VirtualEndpointSettings, VirtualEndpointSpec, formats::SAMPLE_RATES,
};

use crate::{
    DriverBackend, DriverCommand, DriverDeviceConfig, DriverError, DriverProcessId, DriverRuntime,
    DriverSnapshot, DriverState, NativeDirection, SettingsStore, StoredSettings,
};

/// Single non-real-time owner of Driver inventory, revisions and independent settings storage.
pub struct DriverManager {
    clock: Box<dyn mixrelay_time::MonotonicClock>,
    owner: DriverProcessId,
    latency: crate::latency::LatencyAuthority,
    backend: Box<dyn DriverBackend>,
    store: Box<dyn SettingsStore>,
    retained: StoredSettings,
    settings: VirtualEndpointSettings,
    installed_version: Option<String>,
    persistence_error: Option<String>,
    initial_inventory_pending: bool,
}

/// Independent results of lease expiry and native inventory maintenance.
#[derive(Debug)]
pub struct DriverMaintenance {
    /// A failed latency clear does not prevent inventory recovery.
    pub latency: Result<(), DriverError>,
    /// Whether retained native inventory was restored.
    pub recovery: Result<bool, DriverError>,
}

impl DriverManager {
    /// Applies an ephemeral runtime message without persisting or advancing inventory revision.
    /// # Errors
    /// Returns validation or native publication errors.
    pub fn publish_latency(
        &mut self,
        command: crate::LatencyCommand,
    ) -> Result<crate::LatencyReply, DriverError> {
        self.latency
            .command(command, self.clock.now(), self.backend.as_mut())
    }

    /// Loads retained settings and initializes the native inventory only when `backend` is ready.
    /// An absent store gets the fixed duplex default; an existing empty store remains empty.
    /// A non-ready backend never replaces devices or writes new defaults.
    /// # Errors
    /// Returns malformed store, invalid endpoint, native apply or readback errors. Save failures
    /// after successful native apply are retained in the returned manager's snapshot.
    pub fn open(
        backend: impl DriverBackend + 'static,
        store: impl SettingsStore + 'static,
        runtime: DriverRuntime,
    ) -> Result<Self, DriverError> {
        let retained = store.load()?.unwrap_or_else(|| StoredSettings {
            revision: 0,
            active: true,
            devices: vec![default_device()],
        });
        validate_inventory(&retained.devices)?;
        let mut manager = Self {
            clock: runtime.clock,
            owner: runtime.owner,
            backend: Box::new(backend),
            store: Box::new(store),
            retained,
            settings: unavailable_settings(),
            installed_version: None,
            persistence_error: None,
            initial_inventory_pending: true,
            latency: crate::latency::LatencyAuthority::new(runtime.generations),
        };
        manager.initialize()?;
        Ok(manager)
    }

    /// Applies retained startup intent once native ownership becomes available.
    /// Returns false while waiting, and true after native apply and readback.
    /// # Errors
    /// Returns native apply/readback errors without replacing or saving startup intent.
    pub(crate) fn initialize(&mut self) -> Result<bool, DriverError> {
        if !self.initial_inventory_pending {
            return Ok(true);
        }
        if self.backend.state() != DriverState::Ready {
            return Ok(false);
        }
        self.backend.replace(if self.retained.active {
            &self.retained.devices
        } else {
            &[]
        })?;
        let settings = self.backend.settings()?;
        let persisted = self.observe(settings, self.retained.active)?;
        self.initial_inventory_pending = false;
        if !persisted {
            self.persist();
        }
        Ok(true)
    }

    /// Expires latency leases and advances startup or native owner recovery.
    /// Both results remain observable even when either operation fails.
    pub fn maintain(&mut self) -> DriverMaintenance {
        self.maintain_at(self.clock.now())
    }

    fn maintain_at(&mut self, now: mixrelay_time::Instant) -> DriverMaintenance {
        DriverMaintenance {
            latency: self.latency.expire(now, self.backend.as_mut()),
            recovery: self.recover_inventory(),
        }
    }

    fn recover_inventory(&mut self) -> Result<bool, DriverError> {
        if self.initial_inventory_pending {
            return self.initialize();
        }
        if !self.retained.active {
            return Ok(false);
        }
        let recovered = self.backend.recover(&self.retained.devices)?;
        if recovered {
            self.refresh()?;
        }
        Ok(recovered)
    }

    /// Sets the installed service release version; native readiness remains backend-owned.
    pub fn set_installed_version(&mut self, version: Option<String>) {
        self.installed_version = version;
    }

    /// Refreshes actual native state, including external OS format changes.
    /// # Errors
    /// Returns native readback or revision exhaustion errors. Non-ready snapshots disable native
    /// controls when discovery fails rather than synthesizing formats from persisted settings.
    pub fn snapshot(&mut self) -> Result<DriverSnapshot, DriverError> {
        self.refresh()?;
        Ok(self.current_snapshot())
    }

    /// Applies one revision-checked change, reads actual native state and then persists it.
    /// Persistence failure is visible in a successful active-state snapshot, never fake rollback.
    /// # Errors
    /// Rejects stale revisions, missing identities, unavailable backends, invalid requests and
    /// native apply/readback failures. Adapter apply errors preserve the active inventory.
    pub fn mutate(&mut self, command: DriverCommand) -> Result<DriverSnapshot, DriverError> {
        self.refresh()?;
        if self.initial_inventory_pending {
            return Err(DriverError::Unavailable(
                "Driver inventory initialization is pending".into(),
            ));
        }
        if self.backend.state() != DriverState::Ready {
            return Err(DriverError::Unavailable(format!(
                "{:?}",
                self.backend.state()
            )));
        }
        if command.expected_revision() != self.retained.revision {
            return Err(DriverError::RevisionMismatch {
                expected: command.expected_revision(),
                current: self.retained.revision,
            });
        }
        match command {
            DriverCommand::Activate { .. } => return self.set_active(true),
            DriverCommand::Deactivate { .. } => return self.set_active(false),
            _ if !self.retained.active => {
                return Err(DriverError::Unavailable("Driver is stopped".into()));
            }
            _ => {}
        }
        let desired = self.desired(command)?;
        validate_inventory(&desired)?;
        let revision = next_revision(self.retained.revision)?;
        self.backend.replace(&desired)?;
        // Native apply has committed. A readback/save failure cannot pretend it rolled back.
        self.retained.revision = revision;
        self.retained.devices = desired;
        self.settings = self.backend.settings().inspect_err(|error| {
            self.persistence_error = Some(format!(
                "Native change committed, but settings were not saved: {error}"
            ));
        })?;
        self.retained.devices = observed_inventory(&self.settings);
        self.persist();
        Ok(self.current_snapshot())
    }

    fn set_active(&mut self, active: bool) -> Result<DriverSnapshot, DriverError> {
        if active != self.retained.active {
            let revision = next_revision(self.retained.revision)?;
            self.backend
                .replace(if active { &self.retained.devices } else { &[] })?;
            // Keep desired settings independent of an intentionally empty native readback.
            // A readback/save failure must not let maintenance revive stopped devices.
            self.retained.active = active;
            self.retained.revision = revision;
        }
        self.settings = self.backend.settings()?;
        if active {
            self.retained.devices = observed_inventory(&self.settings);
        } else if !self.settings.devices.is_empty() {
            return Err(DriverError::Backend("Driver devices did not stop".into()));
        }
        self.persist();
        if let Some(error) = &self.persistence_error {
            return Err(DriverError::Unavailable(format!(
                "Driver run state was not saved: {error}"
            )));
        }
        Ok(self.current_snapshot())
    }

    fn refresh(&mut self) -> Result<bool, DriverError> {
        // Startup intent is authoritative until its first native ACK/readback.
        // An empty observation before that point must never become a saved empty inventory.
        if self.initial_inventory_pending {
            return Ok(false);
        }
        let ready = self.backend.state() == DriverState::Ready;
        let settings = match self.backend.settings() {
            Ok(settings) => settings,
            Err(_) if !ready => unavailable_settings(),
            Err(error) => return Err(error),
        };
        self.observe(settings, ready && self.retained.active)
    }

    fn observe(
        &mut self,
        settings: VirtualEndpointSettings,
        ready: bool,
    ) -> Result<bool, DriverError> {
        if settings != self.settings {
            self.retained.revision = next_revision(self.retained.revision)?;
            self.settings = settings;
        }
        if ready {
            let actual = observed_inventory(&self.settings);
            if actual != self.retained.devices {
                self.retained.devices = actual;
                self.persist();
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn current_snapshot(&self) -> DriverSnapshot {
        let native_state = self.backend.state();
        let state = if self.initial_inventory_pending && native_state == DriverState::Ready {
            DriverState::Unavailable
        } else {
            native_state
        };
        DriverSnapshot {
            protocol_version: 1,
            revision: self.retained.revision,
            state,
            installed_version: self.installed_version.clone(),
            owner_process_id: Some(self.owner.get()),
            settings: self.settings.clone(),
            persistence_error: self.persistence_error.clone(),
        }
    }

    fn persist(&mut self) {
        self.persistence_error = self
            .store
            .save(&self.retained)
            .err()
            .map(|error| error.to_string());
    }

    fn desired(&self, command: DriverCommand) -> Result<Vec<DriverDeviceConfig>, DriverError> {
        let mut desired = self.retained.devices.clone();
        match command {
            DriverCommand::Activate { .. } | DriverCommand::Deactivate { .. } => {
                unreachable!("run-state commands are handled before inventory commands");
            }
            DriverCommand::Create { spec, .. } => {
                if !self.settings.can_create {
                    return Err(DriverError::Invalid(
                        "native device creation is unsupported".into(),
                    ));
                }
                if desired.iter().any(|device| device.spec.id == spec.id) {
                    return Err(DriverError::Invalid("endpoint ID already exists".into()));
                }
                self.validate_geometry(&spec)?;
                desired.push(DriverDeviceConfig {
                    spec,
                    render_format: None,
                    capture_format: None,
                });
            }
            DriverCommand::Configure { spec, .. } => {
                let device = find_device(&mut desired, &spec.id)?;
                if spec.display_name != device.spec.display_name && !self.settings.can_rename {
                    return Err(DriverError::Invalid(
                        "native renaming is unsupported".into(),
                    ));
                }
                self.validate_geometry(&spec)?;
                reconfigure(device, spec);
            }
            DriverCommand::Remove { endpoint_id, .. } => {
                let index = desired
                    .iter()
                    .position(|device| device.spec.id == endpoint_id)
                    .ok_or(DriverError::NotFound(endpoint_id))?;
                desired.remove(index);
            }
            DriverCommand::SetFormat {
                endpoint_id,
                direction,
                format,
                ..
            } => {
                self.validate_format(&endpoint_id, direction, format)?;
                let device = find_device(&mut desired, &endpoint_id)?;
                set_format(device, direction, format)?;
                self.validate_device_formats(device)?;
            }
            DriverCommand::SetSampleFormat {
                endpoint_id,
                sample_format,
                ..
            } => {
                let device = find_device(&mut desired, &endpoint_id)?;
                self.set_sample_format(device, sample_format)?;
            }
        }
        Ok(desired)
    }

    fn set_sample_format(
        &self,
        device: &mut DriverDeviceConfig,
        sample_format: NativeSampleFormat,
    ) -> Result<(), DriverError> {
        let actual = self
            .settings
            .devices
            .iter()
            .find(|actual| actual.spec.id == device.spec.id)
            .ok_or_else(|| DriverError::NotFound(device.spec.id.clone()))?;
        let mut formats = [None; 2];
        for (index, (stream, native)) in [
            (actual.spec.render, &actual.native_formats.render),
            (actual.spec.capture, &actual.native_formats.capture),
        ]
        .into_iter()
        .enumerate()
        {
            if stream.is_none() {
                continue;
            }
            let current = native.current.ok_or_else(|| {
                DriverError::Invalid("current native format is unavailable".into())
            })?;
            let format = NativeStreamFormat {
                sample_format,
                ..current
            };
            if !native.supported.contains(&format) {
                return Err(DriverError::Invalid(
                    "native format tuple is unsupported".into(),
                ));
            }
            formats[index] = Some(format);
        }
        device.render_format = formats[0];
        device.capture_format = formats[1];
        Ok(())
    }

    fn validate_geometry(&self, spec: &VirtualEndpointSpec) -> Result<(), DriverError> {
        validate_spec(spec)?;
        for (stream, supported) in [
            (spec.render, &self.settings.render_channels),
            (spec.capture, &self.settings.capture_channels),
        ] {
            if stream.is_some_and(|stream| !supported.contains(&stream.channels)) {
                return Err(DriverError::Invalid(
                    "unsupported directional channel count".into(),
                ));
            }
        }
        if let Some(template) = &self.settings.creation_template
            && spec
                .supported_sample_rates
                .iter()
                .any(|rate| !template.supported_sample_rates.contains(rate))
        {
            return Err(DriverError::Invalid(
                "unsupported nominal sample rate".into(),
            ));
        }
        Ok(())
    }

    fn validate_format(
        &self,
        id: &VirtualEndpointId,
        direction: NativeDirection,
        format: NativeStreamFormat,
    ) -> Result<(), DriverError> {
        let device = self
            .settings
            .devices
            .iter()
            .find(|device| &device.spec.id == id)
            .ok_or_else(|| DriverError::NotFound(id.clone()))?;
        let formats = match direction {
            NativeDirection::Render => &device.native_formats.render,
            NativeDirection::Capture => &device.native_formats.capture,
        };
        if !formats.supported.contains(&format) {
            return Err(DriverError::Invalid(
                "native format tuple is unsupported".into(),
            ));
        }
        Ok(())
    }

    fn validate_device_formats(&self, device: &DriverDeviceConfig) -> Result<(), DriverError> {
        for (direction, format) in [
            (NativeDirection::Render, device.render_format),
            (NativeDirection::Capture, device.capture_format),
        ] {
            if let Some(format) = format {
                self.validate_format(&device.spec.id, direction, format)?;
            }
        }
        Ok(())
    }
}

fn next_revision(revision: u64) -> Result<u64, DriverError> {
    revision
        .checked_add(1)
        .ok_or_else(|| DriverError::Invalid("Driver revision exhausted".into()))
}

fn validate_spec(spec: &VirtualEndpointSpec) -> Result<(), DriverError> {
    spec.validate_contract(64, 64)
        .map_err(|error| DriverError::Invalid(error.to_string()))?;
    if spec
        .supported_sample_rates
        .iter()
        .any(|rate| !SAMPLE_RATES.contains(rate))
    {
        return Err(DriverError::Invalid(
            "unsupported product sample rate".into(),
        ));
    }
    Ok(())
}

fn validate_inventory(devices: &[DriverDeviceConfig]) -> Result<(), DriverError> {
    let mut ids = BTreeSet::new();
    for device in devices {
        validate_spec(&device.spec)?;
        if !ids.insert(&device.spec.id) {
            return Err(DriverError::Invalid("duplicate endpoint ID".into()));
        }
        for (stream, format) in [
            (device.spec.render, device.render_format),
            (device.spec.capture, device.capture_format),
        ] {
            if let Some(format) = format
                && !stream.is_some_and(|stream| {
                    stream.channels == format.channels
                        && device.spec.default_sample_rate == format.sample_rate
                })
            {
                return Err(DriverError::Invalid(
                    "native format disagrees with directional geometry or shared nominal rate"
                        .into(),
                ));
            }
        }
    }
    Ok(())
}

fn find_device<'a>(
    devices: &'a mut [DriverDeviceConfig],
    id: &VirtualEndpointId,
) -> Result<&'a mut DriverDeviceConfig, DriverError> {
    devices
        .iter_mut()
        .find(|device| &device.spec.id == id)
        .ok_or_else(|| DriverError::NotFound(id.clone()))
}

fn reconfigure(device: &mut DriverDeviceConfig, spec: VirtualEndpointSpec) {
    for (stream, format) in [
        (spec.render, &mut device.render_format),
        (spec.capture, &mut device.capture_format),
    ] {
        *format = stream.and_then(|stream| {
            format.map(|format| NativeStreamFormat {
                channels: stream.channels,
                sample_rate: spec.default_sample_rate,
                ..format
            })
        });
    }
    device.spec = spec;
}

fn set_format(
    device: &mut DriverDeviceConfig,
    direction: NativeDirection,
    format: NativeStreamFormat,
) -> Result<(), DriverError> {
    let stream = match direction {
        NativeDirection::Render => &mut device.spec.render,
        NativeDirection::Capture => &mut device.spec.capture,
    };
    let stream = stream
        .as_mut()
        .ok_or_else(|| DriverError::Invalid("endpoint direction is absent".into()))?;
    if stream.channels != format.channels {
        stream.channel_layout = ChannelLayout::Discrete;
    }
    stream.channels = format.channels;
    device.spec.default_sample_rate = format.sample_rate;
    for present in [&mut device.render_format, &mut device.capture_format]
        .into_iter()
        .flatten()
    {
        present.sample_rate = format.sample_rate;
    }
    match direction {
        NativeDirection::Render => device.render_format = Some(format),
        NativeDirection::Capture => device.capture_format = Some(format),
    }
    Ok(())
}

fn observed_inventory(settings: &VirtualEndpointSettings) -> Vec<DriverDeviceConfig> {
    settings
        .devices
        .iter()
        .map(|device| {
            let mut spec = device.spec.clone();
            if let Some(rate) = device.observed_sample_rate {
                spec.default_sample_rate = rate;
            }
            DriverDeviceConfig {
                spec,
                render_format: device.native_formats.render.current,
                capture_format: device.native_formats.capture.current,
            }
        })
        .collect()
}

fn unavailable_settings() -> VirtualEndpointSettings {
    VirtualEndpointSettings {
        can_create: false,
        can_rename: false,
        render_channels: Vec::new(),
        capture_channels: Vec::new(),
        creation_template: None,
        devices: Vec::new(),
    }
}

fn default_device() -> DriverDeviceConfig {
    let stream = StreamSpec {
        channels: 2,
        channel_layout: ChannelLayout::Stereo,
    };
    let format = NativeStreamFormat {
        sample_rate: 48_000,
        channels: 2,
        sample_format: NativeSampleFormat::Float32,
    };
    DriverDeviceConfig {
        spec: VirtualEndpointSpec {
            id: VirtualEndpointId::from("17e59d73-42c1-4f11-a8b4-6c2d913357e0"),
            display_name: "MixRelay IO".into(),
            render: Some(stream),
            capture: Some(stream),
            supported_sample_rates: SAMPLE_RATES.to_vec(),
            default_sample_rate: 48_000,
        },
        render_format: Some(format),
        capture_format: Some(format),
    }
}

#[cfg(test)]
mod tests;
