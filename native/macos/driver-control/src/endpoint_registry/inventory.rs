//! Whole-inventory transaction. One reservation excludes mutation until commit/cancel;
//! unchanged transports retain their incarnation and every removed transport is retired.
use super::{
    CallbackEndpoint, EndpointId, EndpointRegistry, EndpointSpec, MappingAllocation,
    NativeMappings, exhausted, require_unreserved,
};
use super::{EndpointMapping as _, OwnedInventory};
use mixrelay_macos_transport::control_inventory::{InventorySnapshot, MAX_ENTRIES};
use std::{collections::BTreeMap, io, sync::Arc};

/// Prepared complete native inventory. Dropping cancels its unpublished changes;
/// callbacks can pin its mappings before a single audio-boundary publication.
#[must_use = "dropping the transaction cancels its inventory changes"]
pub(crate) struct PreparedInventory<'a, A: MappingAllocation = NativeMappings> {
    registry: &'a EndpointRegistry<A>,
    token: Option<u64>,
    next_generation: u64,
    desired: BTreeMap<EndpointId, Arc<A::Mapping>>,
}

impl<A: MappingAllocation> EndpointRegistry<A> {
    /// Prepares exactly the referenced devices; an empty slice removes every endpoint.
    /// Existing equal specs reuse storage. Allocation and validation precede reservation.
    /// # Errors
    /// Rejects duplicate identities, invalid geometry, conflicting mutation, exhausted
    /// generations, or mapping allocation failure without changing active inventory.
    #[cfg(test)]
    pub(crate) fn prepare_inventory(
        &self,
        specs: &[EndpointSpec],
    ) -> io::Result<PreparedInventory<'_, A>> {
        self.prepare_inventory_change(specs, None)
    }

    /// Prepares an inventory, optionally replacing one exhausted mapping regardless of geometry.
    /// # Errors
    /// Rejects an unknown replacement identity and the same failures as `prepare_inventory`.
    pub(crate) fn prepare_inventory_change(
        &self,
        specs: &[EndpointSpec],
        replacement: Option<EndpointId>,
    ) -> io::Result<PreparedInventory<'_, A>> {
        if specs.len() > MAX_ENTRIES {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "endpoint inventory is full",
            ));
        }
        let mut state = self.state.lock();
        require_unreserved(&state)?;
        state
            .retired
            .retain(|mapping| Arc::strong_count(mapping) != 1);
        check_budget(&state, specs, replacement, &self.allocation)?;
        // Commit/cancel must retire pins without another allocation after publication.
        let reserve = specs.len().max(state.active.len());
        state
            .retired
            .try_reserve(reserve)
            .map_err(io::Error::other)?;
        if replacement.is_some_and(|id| {
            !state.active.contains_key(&id) || !specs.iter().any(|spec| spec.id == id)
        }) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "replacement endpoint is absent",
            ));
        }
        let next_generation = state.generation.checked_add(1).ok_or_else(exhausted)?;
        let token = state.next_token;
        state.next_token = token.checked_add(1).ok_or_else(exhausted)?;
        let mut desired = BTreeMap::new();
        for spec in specs {
            spec.validate()?;
            if desired.contains_key(&spec.id) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "duplicate endpoint identity",
                ));
            }
            let mapping = if let Some(current) = state
                .active
                .get(&spec.id)
                .filter(|mapping| mapping.spec() == spec && replacement != Some(spec.id))
            {
                Arc::clone(current)
            } else {
                let generation = state.next_token;
                state.next_token = generation.checked_add(1).ok_or_else(exhausted)?;
                let metadata = state
                    .active
                    .get(&spec.id)
                    .filter(|_| replacement != Some(spec.id))
                    .and_then(|mapping| mapping.reconfigured(spec.clone(), generation));
                Arc::new(match metadata {
                    Some(mapping) => mapping,
                    None => self.allocation.create(spec.clone(), generation)?,
                })
            };
            desired.insert(spec.id, mapping);
        }
        state.inventory_pending = Some(token);
        Ok(PreparedInventory {
            registry: self,
            token: Some(token),
            next_generation,
            desired,
        })
    }
}

fn check_budget<A: MappingAllocation>(
    state: &super::State<A::Mapping>,
    specs: &[EndpointSpec],
    replacement: Option<EndpointId>,
    allocation: &A,
) -> io::Result<()> {
    use super::budget::{Budget, MAX_MAPPINGS, MAX_REGION_BYTES};
    let mut budget = Budget::new(MAX_MAPPINGS, MAX_REGION_BYTES);
    for mapping in state.active.values().chain(&state.retired) {
        let (region, bytes) = mapping.region_usage();
        budget.reserve(Some(region), bytes)?;
    }
    for spec in specs {
        spec.validate()?;
        let current = state
            .active
            .get(&spec.id)
            .filter(|_| replacement != Some(spec.id));
        if current.is_some_and(|mapping| mapping.spec() == spec) {
            continue; // The exact Arc owner is reused, not another mapping.
        }
        if let Some(current) = current.filter(|mapping| mapping.can_reconfigure(spec)) {
            let (region, bytes) = current.region_usage();
            budget.reserve(Some(region), bytes)?;
        } else {
            budget.reserve(None, allocation.required_bytes(spec)?)?;
        }
    }
    Ok(())
}

impl PreparedInventory<'_> {
    /// Duplicates provisional descriptors for HAL preparation before commit.
    /// # Errors
    /// Returns descriptor duplication errors without publishing partial inventory.
    pub(crate) fn snapshot(&self) -> io::Result<OwnedInventory> {
        let mut snapshot = InventorySnapshot {
            generation: self.next_generation,
            ..InventorySnapshot::default()
        };
        let mut descriptors = Vec::with_capacity(self.desired.len());
        for (index, mapping) in self.desired.values().enumerate() {
            let (entry, descriptor) = mapping.inventory_entry()?;
            snapshot.entries[index] = entry;
            descriptors.push(descriptor);
        }
        snapshot.count = descriptors.len();
        Ok(OwnedInventory {
            snapshot,
            _descriptors: descriptors,
        })
    }
}

impl<'a, A: MappingAllocation> PreparedInventory<'a, A> {
    /// Pins each provisional mapping on the control thread for callback publication.
    #[must_use]
    pub(crate) fn callback_pins(&self) -> Vec<CallbackEndpoint<'a, A>> {
        self.desired
            .values()
            .map(|mapping| CallbackEndpoint {
                mapping: Arc::clone(mapping),
                _registry: self.registry,
                _pinned: std::marker::PhantomPinned,
            })
            .collect()
    }

    /// Replaces the entire authoritative inventory under one registry lock/generation.
    /// The reservation excludes every conflicting mutation between preparation and commit.
    /// # Errors
    /// A violated reservation leaves the active inventory unchanged.
    pub(crate) fn commit(mut self) -> io::Result<()> {
        let mut state = self.registry.state.lock();
        if self.token.is_none() || state.inventory_pending != self.token {
            return Err(io::Error::other("endpoint inventory reservation was lost"));
        }
        let previous = std::mem::replace(&mut state.active, std::mem::take(&mut self.desired));
        for (id, mapping) in previous {
            if !state
                .active
                .get(&id)
                .is_some_and(|current| Arc::ptr_eq(current, &mapping))
            {
                state.retired.push(mapping);
            }
        }
        state.generation = self.next_generation;
        state.inventory_pending = None;
        self.token = None;
        Ok(())
    }
}

impl<A: MappingAllocation> Drop for PreparedInventory<'_, A> {
    fn drop(&mut self) {
        let Some(token) = self.token.take() else {
            return;
        };
        let mut state = self.registry.state.lock();
        if state.inventory_pending == Some(token) {
            state.inventory_pending = None;
        }
        for (id, mapping) in std::mem::take(&mut self.desired) {
            if !state
                .active
                .get(&id)
                .is_some_and(|current| Arc::ptr_eq(current, &mapping))
            {
                state.retired.push(mapping);
            }
        }
    }
}
