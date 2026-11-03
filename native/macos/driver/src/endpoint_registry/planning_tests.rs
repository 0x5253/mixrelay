use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Region(Arc<AtomicUsize>);
impl Drop for Region {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
struct MemoryMapping {
    spec: EndpointSpec,
    generation: u64,
    region: Arc<Region>,
}
impl EndpointMapping for MemoryMapping {
    fn spec(&self) -> &EndpointSpec {
        &self.spec
    }
    fn generation(&self) -> u64 {
        self.generation
    }
    fn reconfigured(&self, spec: EndpointSpec, generation: u64) -> Option<Self> {
        self.can_reconfigure(&spec).then(|| Self {
            spec,
            generation,
            region: Arc::clone(&self.region),
        })
    }
    fn can_reconfigure(&self, spec: &EndpointSpec) -> bool {
        (
            self.spec.render_channels,
            self.spec.capture_channels,
            self.spec.capacity_frames,
        ) == (
            spec.render_channels,
            spec.capture_channels,
            spec.capacity_frames,
        )
    }
    fn region_usage(&self) -> (usize, usize) {
        (Arc::as_ptr(&self.region) as usize, 128)
    }
}
struct MemoryAllocation {
    attempts: AtomicUsize,
    fail_at: AtomicUsize,
    live: Arc<AtomicUsize>,
}
impl MemoryAllocation {
    fn new() -> Self {
        Self {
            attempts: AtomicUsize::new(0),
            fail_at: AtomicUsize::new(usize::MAX),
            live: Arc::default(),
        }
    }
}
impl MappingAllocation for MemoryAllocation {
    type Mapping = MemoryMapping;
    fn required_bytes(&self, _: &EndpointSpec) -> io::Result<usize> {
        Ok(128)
    }
    fn create(&self, spec: EndpointSpec, generation: u64) -> io::Result<MemoryMapping> {
        let attempt = self.attempts.fetch_add(1, Ordering::Relaxed);
        if attempt == self.fail_at.load(Ordering::Relaxed) {
            return Err(io::Error::other("allocation failed"));
        }
        self.live.fetch_add(1, Ordering::Relaxed);
        Ok(MemoryMapping {
            spec,
            generation,
            region: Arc::new(Region(Arc::clone(&self.live))),
        })
    }
}
fn spec(id: u8) -> EndpointSpec {
    EndpointSpec {
        id: [id; 16],
        display_name: format!("Device {id}"),
        sample_rate: 48_000,
        capacity_frames: 128,
        render_channels: 2,
        capture_channels: 2,
    }
}

#[test]
fn allocation_failure_preserves_the_active_inventory_and_releases_partial_preparations() {
    let registry = EndpointRegistry::with_allocation(MemoryAllocation::new());
    registry
        .prepare_inventory(&[spec(1)])
        .unwrap()
        .commit()
        .unwrap();
    let original = registry.callback([1; 16]).unwrap();
    registry.allocation.fail_at.store(2, Ordering::Relaxed);
    assert!(
        registry
            .prepare_inventory(&[spec(1), spec(2), spec(3)])
            .is_err()
    );
    assert_eq!(registry.state.lock().active.len(), 1);
    assert!(registry.state.lock().inventory_pending.is_none());
    assert_eq!(registry.allocation.live.load(Ordering::Relaxed), 1);
    assert_eq!(
        registry.callback([1; 16]).unwrap().generation(),
        original.generation()
    );
    registry
        .allocation
        .fail_at
        .store(usize::MAX, Ordering::Relaxed);
    registry
        .prepare_inventory(&[spec(1), spec(2), spec(3)])
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(registry.state.lock().active.len(), 3);
}

#[test]
fn cancellation_retains_provisional_pins_until_control_collects_them() {
    let registry = EndpointRegistry::with_allocation(MemoryAllocation::new());
    let prepared = registry.prepare_inventory(&[spec(1)]).unwrap();
    let pins = prepared.callback_pins();
    assert!(registry.prepare_inventory(&[spec(2)]).is_err());
    drop(prepared);
    assert!(registry.callback([1; 16]).is_none());
    assert_eq!(registry.collect(), 0);
    assert_eq!(registry.allocation.live.load(Ordering::Relaxed), 1);
    drop(pins);
    assert_eq!(registry.collect(), 1);
    assert_eq!(registry.allocation.live.load(Ordering::Relaxed), 0);
    registry
        .prepare_inventory(&[spec(2)])
        .unwrap()
        .commit()
        .unwrap();
    assert!(registry.callback([2; 16]).is_some());
}

#[test]
fn metadata_reuses_storage_but_a_forced_replacement_preserves_the_old_pin() {
    let registry = EndpointRegistry::with_allocation(MemoryAllocation::new());
    registry
        .prepare_inventory(&[spec(1)])
        .unwrap()
        .commit()
        .unwrap();
    let old = registry.callback([1; 16]).unwrap();
    let mut renamed = spec(1);
    renamed.display_name = "Renamed".into();
    registry
        .prepare_inventory(&[renamed.clone()])
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(registry.allocation.attempts.load(Ordering::Relaxed), 1);
    registry
        .prepare_inventory_change(&[renamed], Some([1; 16]))
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(registry.allocation.live.load(Ordering::Relaxed), 2);
    assert_eq!(old.spec().display_name, "Device 1");
    assert_eq!(
        registry.callback([1; 16]).unwrap().spec().display_name,
        "Renamed"
    );
    registry.collect();
    assert_eq!(registry.allocation.live.load(Ordering::Relaxed), 2);
    drop(old);
    registry.collect();
    assert_eq!(registry.allocation.live.load(Ordering::Relaxed), 1);
}
