use super::*;
use crate::plan::{requested_formats, tests::config};
use mixrelay_audio_io::{NativeDeviceFormats, NativeStreamFormats};
use std::{cell::RefCell, collections::BTreeSet, rc::Rc};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("scripted native failure")]
struct Failure;

#[derive(Default)]
struct Lab {
    events: Vec<String>,
    live: BTreeMap<String, usize>,
    next: usize,
    prepares: usize,
    opens: usize,
    pending: usize,
    reject_prepare: BTreeSet<usize>,
    reject_open: BTreeSet<usize>,
    failed: BTreeSet<String>,
    not_ready: BTreeSet<String>,
    reject_read: bool,
}
#[derive(Clone, Default)]
struct Devices(Rc<RefCell<Lab>>);
struct Prepared {
    config: DriverDeviceConfig,
    lab: Rc<RefCell<Lab>>,
}
struct Owner {
    config: DriverDeviceConfig,
    serial: usize,
    lab: Rc<RefCell<Lab>>,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        self.lab.borrow_mut().pending -= 1;
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        let mut lab = self.lab.borrow_mut();
        assert_eq!(
            lab.live.remove(self.config.spec.id.as_str()),
            Some(self.serial)
        );
        lab.events.push(format!(
            "drain:{}:{}",
            self.config.spec.id.as_str(),
            self.serial
        ));
    }
}
impl DeviceOwners for Devices {
    type Error = Failure;
    type Prepared = Prepared;
    type Owner = Owner;
    fn prepare(&self, plan: &DevicePlan) -> Result<Prepared, Failure> {
        let mut lab = self.0.borrow_mut();
        lab.prepares += 1;
        if lab.reject_prepare.contains(&lab.prepares) {
            return Err(Failure);
        }
        lab.pending += 1;
        Ok(Prepared {
            config: plan.config.clone(),
            lab: self.0.clone(),
        })
    }
    fn open(&self, prepared: Prepared) -> Result<Owner, Failure> {
        let mut lab = self.0.borrow_mut();
        lab.opens += 1;
        if lab.reject_open.contains(&lab.opens) {
            drop(lab);
            return Err(Failure);
        }
        let id = prepared.config.spec.id.as_str();
        assert!(
            !lab.live.contains_key(id),
            "duplicate live acquisition: {id}"
        );
        lab.next += 1;
        let serial = lab.next;
        lab.live.insert(id.into(), serial);
        lab.events.push(format!("open:{id}:{serial}"));
        lab.failed.remove(id);
        let owner = Owner {
            config: prepared.config.clone(),
            serial,
            lab: self.0.clone(),
        };
        drop(lab);
        Ok(owner)
    }
}
impl DeviceOwner for Owner {
    type Error = Failure;
    fn config(&self) -> &DriverDeviceConfig {
        &self.config
    }
    fn failed(&self) -> bool {
        self.lab
            .borrow()
            .failed
            .contains(self.config.spec.id.as_str())
    }
    fn ready(&self) -> bool {
        !self.failed()
            && !self
                .lab
                .borrow()
                .not_ready
                .contains(self.config.spec.id.as_str())
    }
    fn settings(&self) -> Result<VirtualEndpointDeviceSettings, Failure> {
        if self.lab.borrow().reject_read {
            return Err(Failure);
        }
        let [render, capture] = requested_formats(&self.config);
        Ok(VirtualEndpointDeviceSettings {
            spec: self.config.spec.clone(),
            native_device_ids: vec![self.serial.to_string()],
            observed_sample_rate: Some(48_000),
            supported_sample_rates: vec![48_000],
            can_set_sample_rate: self.ready(),
            native_formats: NativeDeviceFormats {
                render: NativeStreamFormats {
                    current: render,
                    supported: render.into_iter().collect(),
                },
                capture: NativeStreamFormats {
                    current: capture,
                    supported: capture.into_iter().collect(),
                },
            },
        })
    }
}

fn fixture() -> (Inventory<Devices>, Devices, [DriverDeviceConfig; 3]) {
    let devices = Devices::default();
    let desired = [config("a", 2), config("b", 2), config("healthy", 2)];
    let mut inventory = Inventory::new(devices.clone());
    inventory.replace(&desired).unwrap();
    (inventory, devices, desired)
}

#[test]
fn all_validation_precedes_preparation_and_normalized_readback_keeps_owners() {
    let (mut inventory, devices, mut desired) = fixture();
    let before = inventory.settings().unwrap();
    let prepares = devices.0.borrow().prepares;
    assert!(matches!(
        inventory.replace(&[desired[0].clone(), desired[0].clone()]),
        Err(ReconcileError::Duplicate)
    ));
    desired[1].spec.default_sample_rate = 0;
    assert!(matches!(
        inventory.replace(&desired),
        Err(ReconcileError::Invalid(_))
    ));
    assert_eq!(devices.0.borrow().prepares, prepares);
    assert_eq!(inventory.settings().unwrap(), before);
    desired[1].spec.default_sample_rate = 48_000;
    for config in &mut desired {
        [config.render_format, config.capture_format] = requested_formats(config);
    }
    inventory.replace(&desired).unwrap();
    assert_eq!(devices.0.borrow().prepares, prepares);
    assert_eq!(inventory.settings().unwrap(), before);
}

#[test]
fn desired_or_rollback_preparation_failure_keeps_live_owners_and_reclaims_preparations() {
    for failed_step in [1, 2] {
        let (mut inventory, devices, mut desired) = fixture();
        let before = inventory.settings().unwrap();
        desired[0] = config("a", 4);
        let next = devices.0.borrow().prepares + failed_step;
        devices.0.borrow_mut().reject_prepare.insert(next);
        assert!(matches!(
            inventory.replace(&desired),
            Err(ReconcileError::Native(Failure))
        ));
        assert_eq!(inventory.settings().unwrap(), before);
        assert_eq!(devices.0.borrow().pending, 0);
        assert_eq!(devices.0.borrow().events.len(), 3);
    }
}

#[test]
fn failed_nth_open_drains_staged_owners_before_rollback_and_preserves_healthy_identity() {
    let (mut inventory, devices, original) = fixture();
    let healthy = inventory.owners["healthy"].serial;
    let desired = [config("a", 4), config("b", 4)];
    let next = devices.0.borrow().opens + 2;
    devices.0.borrow_mut().reject_open.insert(next);
    assert!(matches!(
        inventory.replace(&desired),
        Err(ReconcileError::Native(Failure))
    ));
    assert_eq!(inventory.state(), DriverState::Ready);
    assert_eq!(inventory.owners["healthy"].serial, healthy);
    assert_eq!(inventory.owners["a"].config, original[0]);
    assert_eq!(inventory.owners["b"].config, original[1]);
    assert_eq!(devices.0.borrow().pending, 0);
    let events = &devices.0.borrow().events;
    let staged_drain = events
        .iter()
        .position(|event| event == "drain:a:4")
        .unwrap();
    let restored = events.iter().position(|event| event == "open:a:5").unwrap();
    assert!(staged_drain < restored);
}

#[test]
fn rollback_failure_marks_unavailable_until_explicit_recovery_without_reopening_healthy_owners() {
    let (mut inventory, devices, original) = fixture();
    let healthy = inventory.owners["healthy"].serial;
    let desired = [config("a", 4), config("b", 4), original[2].clone()];
    let opens = devices.0.borrow().opens;
    devices
        .0
        .borrow_mut()
        .reject_open
        .extend([opens + 2, opens + 3]);
    assert!(matches!(
        inventory.replace(&desired),
        Err(ReconcileError::Rollback {
            apply: Failure,
            restore: Failure
        })
    ));
    assert_eq!(inventory.state(), DriverState::Unavailable);
    assert_eq!(devices.0.borrow().pending, 0);
    assert_eq!(inventory.owners.len(), 1);
    let next = devices.0.borrow().opens + 1;
    devices.0.borrow_mut().reject_open.insert(next);
    assert!(matches!(
        inventory.recover(&original),
        Err(ReconcileError::Native(Failure))
    ));
    assert_eq!(inventory.state(), DriverState::Unavailable);
    assert_eq!(devices.0.borrow().pending, 0);
    assert!(inventory.recover(&original).unwrap());
    assert_eq!(inventory.state(), DriverState::Ready);
    assert_eq!(inventory.owners["healthy"].serial, healthy);
    assert!(!inventory.recover(&original).unwrap());
}

#[test]
fn recovery_replaces_only_failed_owners_and_empty_inventory_releases_every_acquisition() {
    let (mut inventory, devices, original) = fixture();
    let healthy = inventory.owners["healthy"].serial;
    let failed = inventory.owners["a"].serial;
    devices.0.borrow_mut().failed.insert("a".into());
    assert!(inventory.recover(&original).unwrap());
    assert_ne!(inventory.owners["a"].serial, failed);
    assert_eq!(inventory.owners["healthy"].serial, healthy);
    assert!(!inventory.recover(&original).unwrap());
    inventory.replace(&[]).unwrap();
    assert!(inventory.settings().unwrap().devices.is_empty());
    assert!(devices.0.borrow().live.is_empty());
    assert!(!inventory.recover(&[]).unwrap());
}

#[test]
fn observation_failure_is_not_replaced_with_saved_formats_and_readiness_is_not_failure() {
    let (mut inventory, devices, original) = fixture();
    devices.0.borrow_mut().reject_read = true;
    assert!(inventory.settings().is_err());
    devices.0.borrow_mut().reject_read = false;
    devices.0.borrow_mut().not_ready.insert("a".into());
    assert_eq!(inventory.state(), DriverState::Unavailable);
    assert!(!inventory.recover(&original).unwrap());
    drop(inventory);
    assert!(devices.0.borrow().live.is_empty());
}
