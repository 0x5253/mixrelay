//! Graph source registries and authenticated attachments behind the dispatch/XPC ABI adapter.
mod dispatch;
mod inventory;
mod source;
#[cfg(test)]
mod tests;
mod virtual_pcm;
mod xpc;
use dispatch::Dispatch;
pub use mixrelay_macos_transport::source_registry::SourceClock;
use source::SourceRegistry;
use std::sync::OnceLock;

static SERVICE: OnceLock<Service> = OnceLock::new();
static START_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
struct Service {
    sources: SourceRegistry,
    virtual_pcm: std::sync::Arc<virtual_pcm::VirtualPcm>,
    inventory: inventory::Inventory,
    peers: parking_lot::Mutex<xpc::Peers>,
    dispatch: parking_lot::Mutex<Option<Dispatch>>,
}

struct SharedClock(std::sync::Arc<dyn mixrelay_time::MonotonicClock>);
impl mixrelay_time::MonotonicClock for SharedClock {
    fn now(&self) -> mixrelay_time::Instant {
        self.0.now()
    }
}

/// The process-owned application registry, retained until process exit.
#[must_use]
pub fn source_registry() -> Option<&'static SourceRegistry> {
    SERVICE.get().map(|service| &service.sources)
}

/// The independent registry for authenticated virtual-device PCM; no application grants.
#[must_use]
pub fn virtual_source_registry() -> Option<&'static SourceRegistry> {
    SERVICE.get().map(|service| &service.virtual_pcm.sources)
}

/// Drains virtual input workers on engine shutdown, outside the graph's realtime owner.
pub fn stop_virtual_sources() {
    if let Some(service) = SERVICE.get() {
        service.inventory.stop();
        service.virtual_pcm.clear();
    }
}

/// Starts graph source XPC for console-owned application producers and HAL PCM grants.
/// The independent Driver service owns inventory and basic audio return path.
pub fn start(
    clock: std::sync::Arc<dyn mixrelay_time::MonotonicClock>,
    timebase: mixrelay_realtime::Timebase,
) -> i32 {
    let _start = START_LOCK.lock();
    if SERVICE.get().is_none()
        && SERVICE
            .set(Service {
                sources: SourceRegistry::new(Box::new(SharedClock(clock.clone())), timebase),
                virtual_pcm: std::sync::Arc::new(virtual_pcm::VirtualPcm::new(
                    Box::new(SharedClock(clock)),
                    timebase,
                )),
                inventory: inventory::Inventory::new(),
                peers: parking_lot::Mutex::new(xpc::Peers::default()),
                dispatch: parking_lot::Mutex::new(None),
            })
            .is_err()
    {
        return 2;
    }
    let Some(service) = SERVICE.get() else {
        return 2;
    };
    let mut dispatch = service.dispatch.lock();
    if dispatch.is_some() {
        return 0;
    }
    let Ok(owner) = Dispatch::start() else {
        return 3;
    };
    *dispatch = Some(owner);
    0
}

/// Observes one graph-owned application's current native clock.
/// Returns false when the listener or identified source is absent.
/// # Safety
/// Output points to writable source clock storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_service_source_clock(token: u64, output: *mut SourceClock) -> bool {
    SERVICE.get().is_some_and(|service| unsafe {
        source::mr_engine_source_registry_clock(
            std::ptr::from_ref(&service.sources).cast_mut(),
            token,
            output,
        )
    })
}
/// Selects one source as the graph clock authority using a bounded atomic update.
/// Returns false when the identified source is absent.
#[unsafe(no_mangle)]
pub extern "C" fn mr_service_set_source_clock_master(token: u64, master: bool) -> bool {
    SERVICE.get().is_some_and(|service| unsafe {
        source::mr_engine_source_registry_set_clock_master(
            std::ptr::from_ref(&service.sources).cast_mut(),
            token,
            master,
        )
    })
}
/// Reads clocked Float32 PCM from the one graph-owned source consumer.
/// Returns the number of valid frames, or zero when source PCM is unavailable.
/// # Safety
/// Samples has frames*channels writable floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_service_read_source_clocked(
    token: u64,
    samples: *mut f32,
    frames: u32,
    channels: u32,
    rate: u32,
) -> u32 {
    SERVICE.get().map_or(0, |service| unsafe {
        source::mr_engine_source_registry_read_clocked(
            std::ptr::from_ref(&service.sources).cast_mut(),
            token,
            samples,
            frames,
            channels,
            rate,
        )
    })
}

/// Reports source normalization once even when its next PCM read is unavailable.
#[unsafe(no_mangle)]
pub extern "C" fn mr_service_take_source_faults(token: u64) -> u64 {
    SERVICE.get().map_or(0, |service| {
        // SAFETY: SERVICE retains the registry; the graph consumes one atomic counter.
        unsafe {
            source::mr_engine_source_registry_take_source_faults(&raw const service.sources, token)
        }
    })
}

#[cfg(feature = "native-lab")]
pub(crate) fn initialized() -> bool {
    SERVICE.get().is_some()
}

/// Releases the lab's dispatch owner before releasing its exclusive process leases.
#[cfg(feature = "native-lab")]
pub fn stop_lab_listener() {
    if let Some(service) = SERVICE.get() {
        service.dispatch.lock().take();
    }
}
