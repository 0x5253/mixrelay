//! Authenticated XPC supplies an authoritative inventory. Validate every shared
//! header before deleting anything; malformed or stale snapshots retain the
//! last working registry. Descriptor ownership never escapes the transaction.
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, c_void};
use std::os::fd::{FromRawFd, OwnedFd};

use mixrelay_macos_transport::control_inventory::{
    InventorySnapshot, mr_hal_control_inventory_snapshot_close,
    mr_hal_control_parse_endpoint_remove, mr_hal_control_parse_inventory_snapshot,
};

use crate::abi::{Geometry, NOT_FOUND, OK};
use crate::registry::Registry;

#[derive(Default)]
pub(crate) struct InventoryState {
    // Generation counters are scoped to an authenticated XPC connection, not
    // coreaudiod's lifetime. Driver service restarts allocate a new counter namespace.
    authority: u64,
    connection: u64,
    // Highest fully reconciled generation and highest validated intent differ
    // when a native allocation fails part-way through a retryable snapshot.
    generation: u64,
    latest_validated: u64,
    entries: BTreeMap<[u8; 16], Option<u64>>,
    // The newest generation whose validation or native apply failed, and its authority.
    rejected: Option<(u64, u64)>,
}

// The bridge owns Registry for coreaudiod's lifetime and invokes these control
// exports only on its serial XPC queue, never from an audio callback.
/// Fence the previous XPC peer before accepting any new inventory.
/// Returns false for an invalid, stale or temporarily unavailable connection.
///
/// # Safety
/// `registry` must be the live factory registry. Calls must be serialized with
/// control-message delivery by the bridge's control queue.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_hal_core_control_connected(
    registry: *mut Registry,
    authority: u64,
) -> bool {
    // SAFETY: The bridge passes its live factory-owned Registry; no destroy can
    // run while its XPC queue is servicing a connection event.
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        return false;
    };
    let Ok(mut state) = registry.inventory.try_lock() else {
        return false;
    };
    if authority == 0 || authority < state.connection {
        return false;
    }
    let changed = state.connection != authority;
    state.connection = authority;
    drop(state);
    if changed {
        registry.reset_latency_authority();
    }
    true
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_hal_core_replacement_request(
    registry: *mut Registry,
    authority: u64,
    endpoint: *mut [u8; 16],
    generation: *mut u64,
) -> bool {
    // SAFETY: The bridge owns the registry and supplies two disjoint, aligned
    // stack outputs for this synchronous call on its serial control queue.
    let (Some(registry), Some(endpoint), Some(generation)) = (
        unsafe { registry.as_ref() },
        unsafe { endpoint.as_mut() },
        unsafe { generation.as_mut() },
    ) else {
        return false;
    };
    let Some(id) = registry.replacement_endpoint() else {
        return false;
    };
    let Ok(state) = registry.inventory.try_lock() else {
        return false;
    };
    if authority != state.connection || authority != state.authority {
        return false;
    }
    let Some(Some(current)) = state.entries.get(&id) else {
        return false;
    };
    *endpoint = id;
    *generation = *current;
    true
}

struct SnapshotOwner(InventorySnapshot);
impl Drop for SnapshotOwner {
    fn drop(&mut self) {
        // SAFETY: Parsing transfers duplicated descriptors into this owner;
        // consumed entries are set to -1, so close releases each remaining FD once.
        unsafe { mr_hal_control_inventory_snapshot_close(&raw mut self.0) };
    }
}

/// How the HAL finished the current inventory generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Settlement {
    /// Every attachment was performed.
    Applied,
    /// No attachment is still pending, but at least one was aborted or failed natively.
    Rejected,
}

/// The validated generation once no attachment is pending; `None` while any is.
pub(crate) fn settled(registry: &Registry, authority: u64) -> Option<(u64, Settlement)> {
    let state = registry.inventory.try_lock().ok()?;
    if authority == 0 || state.connection != authority {
        return None;
    }
    // A snapshot that failed validation or a native apply is final until a newer one.
    if let Some((rejected_authority, generation)) = state.rejected
        && rejected_authority == authority
    {
        return Some((generation, Settlement::Rejected));
    }
    if state.authority != authority
        || state.generation == 0
        || state.generation != state.latest_validated
    {
        return None;
    }
    let mut settlement = Settlement::Applied;
    for id in state.entries.keys() {
        if registry.attachment_failed(*id) {
            settlement = Settlement::Rejected;
        } else if !registry.attachment_applied(*id) {
            return None;
        }
    }
    Some((state.generation, settlement))
}

fn preflight(registry: &Registry, snapshot: &InventorySnapshot) -> bool {
    snapshot.entries[..snapshot.count].iter().all(|entry| {
        let mut geometry = Geometry::default();
        crate::formats::SAMPLE_RATES.contains(&entry.sample_rate)
            // SAFETY: Parsing bounds the array and owns each live descriptor;
            // inspect only borrows it and writes this aligned local geometry.
            && unsafe {
                (registry.native.inspect)(entry.descriptor, entry.mapped_size, &raw mut geometry)
            }
            && geometry.valid()
            && geometry.endpoint_id == entry.endpoint_id
    })
}

fn apply(registry: &Registry, authority: u64, snapshot: &mut InventorySnapshot) -> bool {
    let Ok(mut state) = registry.inventory.try_lock() else {
        return false;
    };
    if authority < state.authority {
        return true;
    }
    if authority == state.authority && snapshot.generation < state.latest_validated {
        return true;
    }
    if authority == state.authority
        && snapshot.generation == state.generation
        && !state
            .entries
            .keys()
            .any(|id| registry.attachment_failed(*id))
    {
        return true;
    }
    if !preflight(registry, snapshot) {
        state.rejected = Some((authority, snapshot.generation));
        return false;
    }
    if authority != state.authority {
        state.authority = authority;
        state.generation = 0;
        // Keep endpoint identities for authoritative absent-device removal,
        // but never compare new-peer generations with the old peer's values.
        state
            .entries
            .values_mut()
            .for_each(|version| *version = None);
    }
    state.latest_validated = snapshot.generation;
    let wanted: BTreeSet<_> = snapshot.entries[..snapshot.count]
        .iter()
        .map(|entry| entry.endpoint_id)
        .collect();
    // Only a fully validated authoritative list can remove devices. Reclaim
    // absent idle slots before additions, so a full registry can replace one
    // device. Devices still owned by clients retire safely and may delay retry.
    if !remove_absent(registry, &mut state, &wanted)
        || !upsert_desired(registry, &mut state, snapshot)
    {
        state.rejected = Some((authority, snapshot.generation));
        return false;
    }
    state.rejected = None;
    state.generation = snapshot.generation;
    drop(state);
    registry.transport_policy(snapshot.transport_compatibility.identifiers());
    true
}

fn remove_absent(
    registry: &Registry,
    state: &mut InventoryState,
    wanted: &BTreeSet<[u8; 16]>,
) -> bool {
    let absent: Vec<_> = state
        .entries
        .keys()
        .filter(|id| !wanted.contains(*id))
        .copied()
        .collect();
    for endpoint in absent {
        let status = registry.remove(endpoint);
        if status != OK && status != NOT_FOUND {
            return false;
        }
        state.entries.remove(&endpoint);
    }
    true
}

fn upsert_desired(
    registry: &Registry,
    state: &mut InventoryState,
    snapshot: &mut InventorySnapshot,
) -> bool {
    for entry in &mut snapshot.entries[..snapshot.count] {
        if state
            .entries
            .get(&entry.endpoint_id)
            .is_some_and(|generation| generation.is_some_and(|value| value == entry.generation))
            && !registry.attachment_failed(entry.endpoint_id)
        {
            continue;
        }
        // SAFETY: The parser bounds and NUL-terminates every fixed-size name.
        let name = unsafe { CStr::from_ptr(entry.display_name.as_ptr()) };
        let Ok(name) = name.to_str() else {
            return false;
        };
        let descriptor = std::mem::replace(&mut entry.descriptor, -1);
        // SAFETY: The parser duplicated this valid FD into SnapshotOwner; taking
        // it and clearing the slot transfers sole close ownership to Attachment.
        let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
        if registry
            .upsert(
                entry.endpoint_id,
                name,
                entry.sample_rate,
                (descriptor, entry.mapped_size),
            )
            .is_err()
        {
            return false;
        }
        // Publication is now real even if a later native allocation fails.
        // Track it immediately so the next authoritative list can remove it,
        // or the same-generation retry can continue without recreating it.
        state
            .entries
            .insert(entry.endpoint_id, Some(entry.generation));
    }
    true
}

/// Reconcile an XPC message only within its announced connection namespace.
/// Returns false for stale authority, malformed data or a retryable native failure.
///
/// # Safety
/// `registry` must remain live and `message` must be the retained XPC event for
/// this call. Delivery and connection announcements use the same serial queue.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_hal_core_control_message(
    registry: *mut Registry,
    authority: u64,
    message: *mut c_void,
) -> bool {
    if authority == 0 {
        return false;
    }
    // SAFETY: The factory-owned registry outlives this control-queue callback.
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        return false;
    };
    if !registry
        .inventory
        .try_lock()
        .is_ok_and(|state| state.connection == authority)
    {
        return false;
    }
    let mut snapshot = SnapshotOwner(InventorySnapshot::default());
    // SAFETY: XPC owns the event for the callback duration. The parser validates
    // its types and fills this exclusively borrowed, aligned output snapshot.
    if unsafe { mr_hal_control_parse_inventory_snapshot(message, &raw mut snapshot.0) } {
        // Validate the runtime latency list before applying any authoritative geometry.
        let Some(latencies) = (unsafe {
            mixrelay_macos_transport::control_inventory::parse_input_latencies(message)
        }) else {
            return false;
        };
        let current = registry.inventory.try_lock().is_ok_and(|state| {
            authority >= state.authority
                && (authority != state.authority || snapshot.0.generation >= state.latest_validated)
        });
        if !apply(registry, authority, &mut snapshot.0) {
            return false;
        }
        if current {
            registry.publish_latencies(&latencies);
        }
        return true;
    }
    let mut endpoint = [0_u8; 16];
    let mut generation = 0_u64;
    // SAFETY: The same live event is borrowed; both outputs are disjoint locals
    // of the exact sizes the parser requires (16 bytes and one u64).
    if !unsafe {
        mr_hal_control_parse_endpoint_remove(message, endpoint.as_mut_ptr(), &raw mut generation)
    } {
        return false;
    }
    let Ok(mut state) = registry.inventory.try_lock() else {
        return false;
    };
    // Only a validated full inventory establishes a new connection's authority.
    if authority != state.authority {
        return false;
    }
    if generation <= state.latest_validated {
        return true;
    }
    let status = registry.remove(endpoint);
    if status != OK && status != NOT_FOUND {
        return false;
    }
    state.entries.remove(&endpoint);
    // A single removal notice is not proof that the rest of a partially
    // applied inventory succeeded. A same-generation snapshot must still run.
    state.latest_validated = generation;
    true
}
