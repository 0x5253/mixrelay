//! Inventory transactions over prepared resources and drain-on-drop native owners.

use crate::plan::{self, DevicePlan, PlanError};
use mixrelay_audio_io::{VirtualEndpointDeviceSettings, VirtualEndpointSettings};
use mixrelay_driver_core::{DriverDeviceConfig, DriverState};
use std::{collections::BTreeMap, error::Error};

/// A live owner must finish every callback before its Drop releases native resources.
pub(super) trait DeviceOwner {
    type Error: Error;
    fn config(&self) -> &DriverDeviceConfig;
    fn failed(&self) -> bool;
    fn ready(&self) -> bool;
    fn settings(&self) -> Result<VirtualEndpointDeviceSettings, Self::Error>;
}

/// Preparation reserves all desired and rollback resources without publishing an owner.
/// Opening consumes its preparation; a failed open reclaims its partial native resources.
pub(super) trait DeviceOwners {
    type Error: Error;
    type Prepared;
    type Owner: DeviceOwner<Error = Self::Error>;
    fn prepare(&self, plan: &DevicePlan) -> Result<Self::Prepared, Self::Error>;
    fn open(&self, prepared: Self::Prepared) -> Result<Self::Owner, Self::Error>;
}

type Owners<D> = BTreeMap<String, <D as DeviceOwners>::Owner>;

#[derive(Debug, thiserror::Error)]
pub(super) enum ReconcileError<E: Error + 'static> {
    #[error("duplicate native device identity")]
    Duplicate,
    #[error(transparent)]
    Invalid(#[from] PlanError),
    #[error(transparent)]
    Native(E),
    #[error("native rollback failed: {restore}; desired apply failed: {apply}")]
    Rollback { apply: E, restore: E },
}

pub(super) struct Inventory<D: DeviceOwners> {
    // Owners drain before their factory and any resources it retains are released.
    owners: Owners<D>,
    unavailable: bool,
    devices: D,
}

impl<D: DeviceOwners> Inventory<D>
where
    D::Error: 'static,
{
    pub(super) fn new(devices: D) -> Self {
        Self {
            owners: BTreeMap::new(),
            unavailable: false,
            devices,
        }
    }

    pub(super) fn settings(&self) -> Result<VirtualEndpointSettings, D::Error> {
        let mut settings = VirtualEndpointSettings::configurable(&plan::capabilities());
        settings.devices = self
            .owners
            .values()
            .map(DeviceOwner::settings)
            .collect::<Result<_, _>>()?;
        Ok(settings)
    }

    pub(super) fn replace(
        &mut self,
        desired: &[DriverDeviceConfig],
    ) -> Result<(), ReconcileError<D::Error>> {
        let mut wanted = BTreeMap::new();
        for config in desired {
            let key = config.spec.id.as_str().to_owned();
            if wanted.contains_key(&key) {
                return Err(ReconcileError::Duplicate);
            }
            wanted.insert(key, DevicePlan::new(config)?);
        }
        let mut prepared = Vec::new();
        let mut previous = Vec::new();
        for (key, plan) in &wanted {
            let current = self.owners.get(key);
            if current.is_some_and(|owner| plan.matches(owner.config()) && !owner.failed()) {
                continue;
            }
            prepared.push((
                key.clone(),
                self.devices.prepare(plan).map_err(ReconcileError::Native)?,
            ));
            if let Some(owner) = current {
                let rollback = DevicePlan::new(owner.config())?;
                previous.push((
                    key.clone(),
                    self.devices
                        .prepare(&rollback)
                        .map_err(ReconcileError::Native)?,
                ));
            }
        }
        // Drop and drain changed owners only after all desired and rollback preparation succeeds.
        for (key, _) in &prepared {
            self.owners.remove(key);
        }
        let staged = self.open_staged(prepared, previous)?;
        self.owners.extend(staged);
        self.owners.retain(|id, _| wanted.contains_key(id));
        self.unavailable = false;
        Ok(())
    }

    fn open_staged(
        &mut self,
        prepared: Vec<(String, D::Prepared)>,
        previous: Vec<(String, D::Prepared)>,
    ) -> Result<Owners<D>, ReconcileError<D::Error>> {
        let mut staged = BTreeMap::new();
        for (key, preparation) in prepared {
            match self.devices.open(preparation) {
                Ok(owner) => {
                    staged.insert(key, owner);
                }
                Err(apply) => {
                    drop(staged); // Drain desired identities before restoring their former owners.
                    for (key, preparation) in previous {
                        match self.devices.open(preparation) {
                            Ok(owner) => {
                                self.owners.insert(key, owner);
                            }
                            Err(restore) => {
                                self.unavailable = true;
                                return Err(ReconcileError::Rollback { apply, restore });
                            }
                        }
                    }
                    return Err(ReconcileError::Native(apply));
                }
            }
        }
        Ok(staged)
    }

    pub(super) fn recover(
        &mut self,
        desired: &[DriverDeviceConfig],
    ) -> Result<bool, ReconcileError<D::Error>> {
        if !self.unavailable && !self.owners.values().any(DeviceOwner::failed) {
            return Ok(false);
        }
        self.replace(desired)?;
        Ok(true)
    }

    pub(super) fn state(&self) -> DriverState {
        if self.unavailable || self.owners.values().any(|owner| !owner.ready()) {
            DriverState::Unavailable
        } else {
            DriverState::Ready
        }
    }

    #[cfg(all(test, target_os = "linux", feature = "native-lab"))]
    pub(super) fn owners(&self) -> &BTreeMap<String, D::Owner> {
        &self.owners
    }
}

#[cfg(test)]
mod tests;
