//! Control-owned virtual endpoint transports, independent of graph identity.
//!
//! A callback receives an immutable pinned handle before rendering starts. It
//! never discovers an endpoint through a global pointer or locks this registry.
//! Retired mappings remain owned here until control-thread collection or preparation.
#[path = "endpoint_registry/budget.rs"]
mod budget;
#[path = "endpoint_registry/inventory.rs"]
mod inventory;
#[path = "endpoint_registry/mapping.rs"]
mod mapping;
#[cfg(test)]
#[path = "endpoint_registry/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "endpoint_registry/planning_tests.rs"]
mod planning_tests;

use mapping::NativeMappings;
#[cfg(test)]
use mixrelay_macos_transport::control_inventory::InventoryEntry;
use mixrelay_macos_transport::control_inventory::{InventorySnapshot, NAME_BYTES};
use mixrelay_macos_transport::endpoint_transport::EndpointTransport;
#[cfg(test)]
use mixrelay_macos_transport::shared_audio_buffer::SharedAudioBuffer;
use parking_lot::Mutex;
use std::{collections::BTreeMap, ffi::CString, io, os::fd::OwnedFd, sync::Arc};

/// Stable UUID bytes shared by the HAL inventory and transport headers.
pub(crate) type EndpointId = [u8; 16];

#[derive(Clone, Debug, PartialEq, Eq)]
/// Control-owned identity, display metadata and immutable transport geometry.
pub(crate) struct EndpointSpec {
    pub(crate) id: EndpointId,
    pub(crate) display_name: String,
    pub(crate) sample_rate: u32,
    pub(crate) capacity_frames: u32,
    pub(crate) render_channels: u32,
    pub(crate) capture_channels: u32,
}

impl EndpointSpec {
    fn validate(&self) -> io::Result<()> {
        if self.id == [0; 16]
            || self.display_name.is_empty()
            || self.display_name.len() >= NAME_BYTES
            || CString::new(self.display_name.as_str()).is_err()
            || !mixrelay_audio_io::formats::SAMPLE_RATES.contains(&self.sample_rate)
            || self.render_channels > 64
            || self.capture_channels > 64
            || self.render_channels + self.capture_channels == 0
            || !self.capacity_frames.is_power_of_two()
            || !(32..=1 << 20).contains(&self.capacity_frames)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid endpoint geometry or identity",
            ));
        }
        Ok(())
    }
}

/// Immutable mapping ownership used by inventory planning and reservation.
pub(super) trait EndpointMapping: Send + Sync + Sized {
    fn spec(&self) -> &EndpointSpec;
    fn generation(&self) -> u64;
    fn reconfigured(&self, spec: EndpointSpec, generation: u64) -> Option<Self>;
    fn can_reconfigure(&self, spec: &EndpointSpec) -> bool;
    fn region_usage(&self) -> (usize, usize);
}

/// Allocation stays outside inventory decisions; fake mappings need no file descriptors.
pub(super) trait MappingAllocation {
    type Mapping: EndpointMapping;
    fn required_bytes(&self, spec: &EndpointSpec) -> io::Result<usize>;
    fn create(&self, spec: EndpointSpec, generation: u64) -> io::Result<Self::Mapping>;
}

struct State<M: EndpointMapping> {
    active: BTreeMap<EndpointId, Arc<M>>,
    retired: Vec<Arc<M>>,
    next_token: u64,
    generation: u64,
    inventory_pending: Option<u64>,
}

/// Owns active and retired mappings; mutations occur only on control threads.
pub(crate) struct EndpointRegistry<A: MappingAllocation = NativeMappings> {
    state: Mutex<State<A::Mapping>>,
    allocation: A,
}

impl EndpointRegistry {
    /// Constructs an empty unpublished device inventory with native mapping allocation.
    pub(crate) fn new() -> Self {
        Self::with_allocation(NativeMappings)
    }
}

impl<A: MappingAllocation> EndpointRegistry<A> {
    fn with_allocation(allocation: A) -> Self {
        Self {
            state: Mutex::new(State {
                active: BTreeMap::new(),
                retired: Vec::new(),
                next_token: 1,
                generation: 1,
                inventory_pending: None,
            }),
            allocation,
        }
    }

    /// Pins on control before passing the handle into a callback. The lifetime
    /// prevents registry destruction while any callback can access its mapping.
    #[must_use]
    pub(crate) fn callback(&self, id: EndpointId) -> Option<CallbackEndpoint<'_, A>> {
        self.state
            .lock()
            .active
            .get(&id)
            .map(|mapping| CallbackEndpoint {
                mapping: Arc::clone(mapping),
                _registry: self,
                _pinned: std::marker::PhantomPinned,
            })
    }

    /// Reclaims retired storage on the control thread; preparation also collects it.
    pub(crate) fn collect(&self) -> usize {
        let mut state = self.state.lock();
        let before = state.retired.len();
        state
            .retired
            .retain(|mapping| Arc::strong_count(mapping) != 1);
        before - state.retired.len()
    }
}

impl EndpointRegistry {
    /// Duplicates every descriptor before exposing an authoritative snapshot.
    /// # Errors
    /// Any descriptor duplication failure drops all copies and returns no snapshot.
    pub(crate) fn snapshot(&self) -> io::Result<OwnedInventory> {
        let state = self.state.lock();
        let mut snapshot = InventorySnapshot {
            generation: state.generation,
            ..InventorySnapshot::default()
        };
        let mut descriptors = Vec::with_capacity(state.active.len());
        for (index, mapping) in state.active.values().enumerate() {
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

fn exhausted() -> io::Error {
    io::Error::other("endpoint generation exhausted; no generation is reused")
}

fn require_unreserved<M: EndpointMapping>(state: &State<M>) -> io::Result<()> {
    if state.inventory_pending.is_some() {
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "endpoint inventory transaction is reserved",
        ))
    } else {
        Ok(())
    }
}

/// No lookup, lock, allocation or final mapping destruction occurs on drop.
/// Even after removal, the registry owns the retired mapping until `collect`.
pub(crate) struct CallbackEndpoint<'registry, A: MappingAllocation = NativeMappings> {
    mapping: Arc<A::Mapping>,
    _registry: &'registry EndpointRegistry<A>,
    // A service may publish this handle's address to callbacks. Once pinned,
    // moving its fields out must remain impossible until the readers drain.
    _pinned: std::marker::PhantomPinned,
}

impl<A: MappingAllocation> CallbackEndpoint<'_, A> {
    #[must_use]
    /// Immutable metadata for this pinned mapping.
    pub(crate) fn spec(&self) -> &EndpointSpec {
        self.mapping.spec()
    }
    #[must_use]
    /// Inventory attachment generation, never a graph revision.
    pub(crate) fn generation(&self) -> u64 {
        self.mapping.generation()
    }
}

impl CallbackEndpoint<'_> {
    #[must_use]
    /// Live shared transport retained by this pin and the registry.
    pub(crate) fn transport(&self) -> *mut EndpointTransport {
        self.mapping.transport()
    }
    #[must_use]
    /// HAL-owned render ring, or null for capture-only devices.
    #[cfg(test)]
    pub(crate) fn render_buffer(&self) -> *mut SharedAudioBuffer {
        self.mapping.render_buffer()
    }
    #[must_use]
    /// HAL-owned mixed capture ring, or null for render-only devices.
    #[cfg(test)]
    pub(crate) fn capture_buffer(&self) -> *mut SharedAudioBuffer {
        self.mapping.capture_buffer()
    }
}

/// The ABI contains borrowed FD numbers; this owner keeps all of them valid
/// across registry replacement/removal until the XPC codec duplicates them.
pub(crate) struct OwnedInventory {
    snapshot: InventorySnapshot,
    _descriptors: Vec<OwnedFd>,
}
impl OwnedInventory {
    pub(crate) fn set_generation(&mut self, generation: u64) {
        self.snapshot.generation = generation;
    }
    #[must_use]
    /// Borrowed ABI snapshot whose descriptor lifetimes are retained by self.
    pub(crate) fn as_abi(&self) -> &InventorySnapshot {
        &self.snapshot
    }
    #[must_use]
    /// Complete authoritative entries in stable identity order.
    #[cfg(test)]
    pub(crate) fn entries(&self) -> &[InventoryEntry] {
        &self.snapshot.entries[..self.snapshot.count]
    }
}
