//! The leases the active plan asks for: each enabled forwarding intent, on an owned
//! Driver render sink, for every capture stream verified to deliver PCM.
use super::{
    OwnedPlan, PlaybackResources,
    lease::{Lease, LeaseKey},
    registry::Registry,
};
use mixrelay_audio_graph::composition::ApplicationPlaybackIntent;
use mixrelay_driver_core::DriverState;
use mixrelay_linux_audio_io::process_identity::{ProcessIdentity, process_identity};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn desired_leases(
    plan: &OwnedPlan,
    resources: &PlaybackResources<'_>,
    registry: &Registry,
) -> Result<BTreeMap<LeaseKey, Lease>, String> {
    let OwnedPlan::Forward {
        intents,
        controls: bank,
    } = plan
    else {
        return Ok(BTreeMap::new());
    };
    if !registry.hook_ready() {
        return Err("WirePlumber source-mute hook disappeared".into());
    }
    let Some(watch) = resources.watch else {
        return Ok(BTreeMap::new());
    };
    let inventory = mixrelay_linux_audio_io::targets().map_err(|e| e.to_string())?;
    let driver = resources
        .driver
        .ok_or("Driver owner snapshot unavailable")?;
    if driver.state != DriverState::Ready {
        return Err("Driver is not ready".into());
    }
    let driver_owner = driver
        .owner_process_id
        .and_then(process_identity)
        .ok_or("Driver process identity unavailable")?;
    let mut desired: BTreeMap<LeaseKey, Lease> = BTreeMap::new();
    for intent in intents {
        if !bank
            .snapshot(&intent.mask_node_id)
            .is_some_and(|(control, _)| control.enabled)
        {
            continue;
        }
        let Some(endpoint) = owned_render(intent, resources, registry, &driver_owner) else {
            continue;
        };
        for target in watch.verified_playback_targets(intent.capture_node_id.as_str(), &inventory) {
            let Some(source_owner) =
                registry.source_owner(target.object_id, &target.serial, &target.process)
            else {
                continue;
            };
            let Some(source_client_serial) = registry.client_serial_for_node(target.object_id)
            else {
                continue;
            };
            let key = LeaseKey {
                endpoint_serial: endpoint.serial.clone(),
                source_serial: target.serial,
            };
            desired
                .entry(key.clone())
                .and_modify(|existing| existing.references += 1)
                .or_insert_with(|| Lease {
                    key,
                    endpoint_id: endpoint.id,
                    source_id: target.object_id,
                    endpoint_process: driver_owner.clone(),
                    source_process: target.process,
                    source_owner,
                    endpoint_client_serial: endpoint.client_serial.clone(),
                    source_client_serial,
                    capture_healthy: target.healthy,
                    capture_fresh: target.fresh_pcm,
                    nonce: String::new(),
                    references: 1,
                    other_sink_serials: BTreeSet::new(),
                    routing_target: None,
                    source_target_hint: None,
                });
        }
    }
    Ok(desired)
}

/// The Driver render sink an intent forwards to.
struct OwnedRender {
    id: u32,
    serial: String,
    client_serial: String,
}

/// The intent's forwarding endpoint is part of this graph and is a live Driver
/// render sink with exactly one registry node under its canonical name.
fn owned_render(
    intent: &ApplicationPlaybackIntent,
    resources: &PlaybackResources<'_>,
    registry: &Registry,
    driver_owner: &ProcessIdentity,
) -> Option<OwnedRender> {
    if !resources
        .endpoints
        .values()
        .any(|spec| spec.id == intent.forwarding_endpoint)
    {
        return None;
    }
    let device = resources.driver.and_then(|snapshot| {
        snapshot
            .settings
            .devices
            .iter()
            .find(|device| device.spec.id == intent.forwarding_endpoint)
    })?;
    if device.spec.render.is_none() || device.native_formats.render.current.is_none() {
        return None;
    }
    let expected_name = format!(
        "{}.render",
        mixrelay_linux_driver::native_name(&device.spec.id)
    );
    let mut candidates = registry.object_ids().filter_map(|id| {
        registry
            .endpoint(id, &expected_name, driver_owner)
            .map(|serial| (id, serial))
    });
    let (id, serial) = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    registry
        .client_serial_for_node(id)
        .map(|client_serial| OwnedRender {
            id,
            serial,
            client_serial,
        })
}
