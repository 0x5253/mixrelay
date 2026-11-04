//! Prepared virtual-input owners; grants hand queues to an existing, paced pump.
mod cadence;
mod converter;
mod pump;
mod scheduling;
pub(super) use pump::Pump;
#[cfg(test)]
pub(super) mod test_storage;
#[cfg(test)]
mod tests;
use mixrelay_macos_transport::{
    application_source_transport::SourceTransport,
    client_pcm::{ClientPcmReader, EndpointGeometry, QUEUE_FRAMES},
    endpoint_identity::format_endpoint_id,
    source_control::SourceRequest,
    source_registry::SourceRegistry,
};
use mixrelay_realtime::{Timebase, host_time};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
};

#[derive(Debug, thiserror::Error)]
pub(super) enum Error {
    #[error("virtual PCM scheduling failed: {0}")]
    Scheduling(std::io::Error),
    #[error("{0}")]
    Snapshot(#[from] mixrelay_macos_transport::client_pcm::SnapshotError),
    #[error("virtual PCM snapshot has inconsistent endpoint geometry")]
    Geometry,
    #[error("virtual PCM source preparation failed: {0}")]
    Prepare(#[from] std::io::Error),
    #[error("virtual PCM conversion failed: {0}")]
    Bridge(#[from] mixrelay_audio_io::clock_bridge::BridgeError),
    #[error("virtual PCM producer grant is stale")]
    Stale,
}
impl Error {
    pub(super) fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Prepare(_)
                | Self::Bridge(mixrelay_audio_io::clock_bridge::BridgeError::Allocation { .. })
        )
    }
}

struct Update {
    queues: Vec<ClientPcmReader>,
    received: u64,
    /// Any empty membership since the pump last consumed an update ends retention.
    end_retention: bool,
}
/// Worker lifetime boundary; tests drive the same pump with a synthetic clock.
pub(super) trait Worker: Sized + Send + 'static {
    fn start(pump: Pump) -> std::io::Result<Self>;
    fn running(&self) -> bool;
    fn wake(&self);
    fn finish(self);
}
pub(super) struct ThreadWorker(thread::JoinHandle<()>);
impl Worker for ThreadWorker {
    fn start(pump: Pump) -> std::io::Result<Self> {
        let (ready, started) = std::sync::mpsc::sync_channel(0);
        let worker = thread::Builder::new().name("mixrelay-virtual-input".into()).spawn(move || {
            if let Err(error) = pump.run(move || { let _ = ready.send(()); }) {
                tracing::error!(boundary = "hal_shared_pcm", stage = "source_pump", %error, "virtual input stopped");
            }
        })?;
        if let Err(error) = started.recv() {
            let _ = mixrelay_engine_host::join_worker(worker);
            return Err(std::io::Error::other(error));
        }
        Ok(Self(worker))
    }
    fn running(&self) -> bool {
        !self.0.is_finished()
    }
    fn wake(&self) {
        self.0.thread().unpark();
    }
    fn finish(self) {
        let _ = mixrelay_engine_host::join_worker(self.0);
    }
}
struct Endpoint<W: Worker> {
    geometry: EndpointGeometry,
    source_token: u64,
    observed: u64,
    grants_at: u64,
    update: Arc<parking_lot::Mutex<Option<Update>>>,
    updated: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: Option<W>,
}
impl<W: Worker> Endpoint<W> {
    fn start(
        sources: &SourceRegistry,
        geometry: EndpointGeometry,
        observed: u64,
    ) -> Result<Self, Error> {
        let source_id = format!("virtual:{}", format_endpoint_id(geometry.id));
        let mut request = SourceRequest {
            channels: geometry.capture_channels,
            capacity_frames: mixrelay_audio_io::retention::RING_FRAMES,
            transport_version: mixrelay_macos_transport::application_clock::VERSION,
            ..SourceRequest::default()
        };
        for (target, byte) in request.source_id.iter_mut().zip(source_id.bytes()) {
            *target = std::ffi::c_char::from_ne_bytes([byte]);
        }
        // SAFETY: the default request retains a terminated canonical source ID.
        let source_token = unsafe {
            mixrelay_macos_transport::source_registry::ffi::mr_engine_source_id_token(
                request.source_id.as_ptr(),
            )
        };
        let source_started = host_time();
        let transport = SourceTransport::create(sources, &request)?;
        let source_finished = host_time();
        let update = Arc::new(parking_lot::Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let updated = Arc::new(AtomicBool::new(false));
        let mut context = pump::Pump::prepare_empty(
            geometry.capture_channels as usize,
            transport,
            pump::Signals {
                update: update.clone(),
                updated: updated.clone(),
                stop: stop.clone(),
            },
        )?;
        context.preparation(source_started, source_finished);
        let worker = W::start(context)?;
        Ok(Self {
            geometry,
            source_token,
            observed,
            grants_at: 0,
            update,
            updated,
            stop,
            worker: Some(worker),
        })
    }
    fn ready(&self, geometry: EndpointGeometry, sources: &SourceRegistry) -> bool {
        self.geometry == geometry
            && self.worker.as_ref().is_some_and(Worker::running)
            && sources.producer_can_rearm(self.source_token)
    }
    fn replace(&self, queues: Vec<ClientPcmReader>, received: u64) {
        let mut update = self.update.lock();
        let end_retention =
            queues.is_empty() || update.as_ref().is_some_and(|update| update.end_retention);
        *update = Some(Update {
            queues,
            received,
            end_retention,
        });
        drop(update);
        // Release publishes the replacement; the pump consumes it only on control.
        self.updated.store(true, Ordering::Release);
        if let Some(worker) = &self.worker {
            worker.wake();
        }
    }
}
impl<W: Worker> Drop for Endpoint<W> {
    fn drop(&mut self) {
        // Release asks the sole pump to finish before control joins and reclaims storage.
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.wake();
            worker.finish();
        }
    }
}

struct Preparation<'a> {
    _guard: parking_lot::MutexGuard<'a, ()>,
    sources: &'a SourceRegistry,
}
impl Drop for Preparation<'_> {
    fn drop(&mut self) {
        self.sources.collect_retired();
    }
}

pub(super) struct PendingGrants {
    generation: u64,
    queues: Vec<ClientPcmReader>,
    members: BTreeSet<[u8; 16]>,
}

pub(super) struct VirtualPcm<W: Worker = ThreadWorker> {
    pub(super) sources: SourceRegistry,
    endpoints: parking_lot::Mutex<BTreeMap<[u8; 16], Endpoint<W>>>,
    /// Serializes preparations, never queue handoff or clear. No duplicate source leases.
    preparing: parking_lot::Mutex<()>,
    /// A preparation crossing clear must never insert an owner.
    cleared: AtomicU64,
    inventory_at: AtomicU64,
    membership_at: AtomicU64,
}
impl VirtualPcm {
    pub(super) fn new(
        clock: Box<dyn mixrelay_time::MonotonicClock>,
        timebase: mixrelay_realtime::Timebase,
    ) -> Self {
        Self::with_sources(SourceRegistry::new(clock, timebase))
    }
}
impl<W: Worker> VirtualPcm<W> {
    fn with_sources(sources: SourceRegistry) -> Self {
        Self {
            sources,
            endpoints: parking_lot::Mutex::new(BTreeMap::new()),
            preparing: parking_lot::Mutex::new(()),
            cleared: AtomicU64::new(0),
            inventory_at: AtomicU64::new(0),
            membership_at: AtomicU64::new(0),
        }
    }
    #[cfg(test)]
    pub(super) fn with_test_sources(sources: SourceRegistry) -> Self {
        Self::with_sources(sources)
    }
    /// Prepares published geometry before clients normally start native IO.
    #[cfg(test)]
    fn configure(&self, geometry: &[EndpointGeometry], observed: u64) -> Result<(), Error> {
        self.configure_inner(geometry, observed, None, 0)
    }
    pub(super) fn configure_pending(
        &self,
        geometry: &[EndpointGeometry],
        pending: &mut PendingGrants,
        observed: u64,
        received: u64,
    ) -> Result<(), Error> {
        self.configure_inner(geometry, observed, Some(pending), received)
    }
    fn configure_inner(
        &self,
        geometry: &[EndpointGeometry],
        observed: u64,
        mut pending: Option<&mut PendingGrants>,
        received: u64,
    ) -> Result<(), Error> {
        validate_geometry(geometry)?;
        let _preparing = Preparation {
            _guard: self.preparing.lock(),
            sources: &self.sources,
        };
        let generation = self.cleared.load(Ordering::Acquire);
        if pending
            .as_ref()
            .is_some_and(|pending| pending.generation != generation)
        {
            return Err(Error::Stale);
        }
        if observed < self.inventory_at.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut endpoints = self.endpoints.lock();
        let retired: Vec<_> = endpoints
            .extract_if(.., |id, endpoint| {
                if observed < endpoint.observed {
                    return false;
                }
                match geometry.iter().find(|entry| entry.id == *id) {
                    Some(shape) => !endpoint.ready(*shape, &self.sources),
                    None => true,
                }
            })
            .map(|(_, endpoint)| endpoint)
            .collect();
        drop(endpoints);
        drop(retired);
        self.sources.collect_retired();
        let mut failure = None;
        for &shape in geometry
            .iter()
            .filter(|shape| shape.capture_channels != 0 && shape.render_channels != 0)
        {
            if let Some(endpoint) = self.endpoints.lock().get_mut(&shape.id) {
                endpoint.observed = endpoint.observed.max(observed);
                continue;
            }
            if self.endpoints.lock().len() == 64 {
                return Err(Error::Geometry);
            }
            let endpoint = Endpoint::start(&self.sources, shape, observed);
            let mut endpoints = self.endpoints.lock();
            if self.cleared.load(Ordering::Acquire) != generation {
                drop(endpoints);
                drop(endpoint);
                self.sources.collect_retired();
                return Err(Error::Stale);
            }
            match endpoint {
                Ok(endpoint) => {
                    endpoints.insert(shape.id, endpoint);
                    drop(endpoints);
                    if let Some(pending) = pending.as_deref_mut() {
                        self.replace_pending(pending, observed, received)?;
                    }
                }
                Err(error) => {
                    drop(endpoints);
                    self.sources.collect_retired();
                    failure.get_or_insert(error);
                }
            }
        }
        let endpoints = self.endpoints.lock();
        if self.cleared.load(Ordering::Acquire) != generation {
            drop(endpoints);
            self.sources.collect_retired();
            return Err(Error::Stale);
        }
        if let Some(error) = failure {
            return Err(error);
        }
        self.inventory_at.fetch_max(observed, Ordering::Release);
        drop(endpoints);
        Ok(())
    }
    /// Hand prepared endpoints their queues before inventory work can wait.
    pub(super) fn replace_ready(
        &self,
        queues: Vec<ClientPcmReader>,
        observed: u64,
        received: u64,
    ) -> Result<PendingGrants, Error> {
        // Acquire captures the clear generation before this snapshot can enqueue work.
        let generation = self.cleared.load(Ordering::Acquire);
        // Release exposes the newest complete membership before a preparer can
        // commit older queues; its Acquire check rejects superseded handoffs.
        self.membership_at.fetch_max(observed, Ordering::Release);
        let members = queues
            .iter()
            .filter(|queue| queue.capture_channels() != 0)
            .map(ClientPcmReader::endpoint)
            .collect();
        let endpoints = self.endpoints.lock();
        let (ready, pending) = queues.into_iter().partition(|queue| {
            endpoints.get(&queue.endpoint()).is_some_and(|endpoint| {
                endpoint.ready(
                    EndpointGeometry {
                        id: queue.endpoint(),
                        render_channels: queue.channels(),
                        capture_channels: queue.capture_channels(),
                    },
                    &self.sources,
                )
            })
        });
        drop(endpoints);
        self.replace(ready, observed, received)?;
        Ok(PendingGrants {
            generation,
            queues: pending,
            members,
        })
    }
    pub(super) fn replace_pending(
        &self,
        pending: &mut PendingGrants,
        observed: u64,
        received: u64,
    ) -> Result<(), Error> {
        // A newer complete membership supersedes queued preparation, including a
        // removal received before this endpoint's storage finished preparation.
        if pending.generation != self.cleared.load(Ordering::Acquire)
            || observed < self.membership_at.load(Ordering::Acquire)
        {
            return Err(Error::Stale);
        }
        let endpoints = self.endpoints.lock();
        let (ready, deferred) =
            std::mem::take(&mut pending.queues)
                .into_iter()
                .partition(|queue| {
                    endpoints
                        .get(&queue.endpoint())
                        .is_some_and(|endpoint| endpoint.ready(endpoint.geometry, &self.sources))
                });
        pending.queues = deferred;
        drop(endpoints);
        self.replace_members(ready, Some(&pending.members), observed, received)
    }
    /// Hands queues to prepared pumps; unprepared grants use the inventory worker.
    pub(super) fn replace(
        &self,
        queues: Vec<ClientPcmReader>,
        observed: u64,
        received: u64,
    ) -> Result<(), Error> {
        self.replace_members(queues, None, observed, received)
    }
    fn replace_members(
        &self,
        queues: Vec<ClientPcmReader>,
        members: Option<&BTreeSet<[u8; 16]>>,
        observed: u64,
        received: u64,
    ) -> Result<(), Error> {
        let mut groups: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for queue in queues {
            if queue.capture_channels() != 0 {
                groups.entry(queue.endpoint()).or_default().push(queue);
            }
        }
        let mut endpoints = self.endpoints.lock();
        if groups.len() > 64
            || groups.iter().any(|(id, queues)| {
                !endpoints.get(id).is_some_and(|endpoint| {
                    observed < endpoint.observed
                        || (endpoint.ready(endpoint.geometry, &self.sources)
                            && queues.len() <= 256
                            && queues.iter().all(|queue| {
                                queue.capture_channels() == endpoint.geometry.capture_channels
                                    && queue.channels() == endpoint.geometry.render_channels
                                    && queue.capacity() == QUEUE_FRAMES
                            }))
                })
            })
        {
            return Err(Error::Geometry);
        }
        for (id, endpoint) in endpoints.iter_mut() {
            if observed < endpoint.grants_at {
                continue;
            }
            let queues = if let Some(queues) = groups.remove(id) {
                queues
            } else if members.is_some_and(|members| members.contains(id)) {
                continue;
            } else {
                Vec::new()
            };
            // Older grants of the current shape remain useful after inventory
            // preparation. Ignore obsolete geometry or membership.
            if queues.iter().any(|queue| {
                queue.capture_channels() != endpoint.geometry.capture_channels
                    || queue.channels() != endpoint.geometry.render_channels
            }) {
                continue;
            }
            if !queues.is_empty() {
                log_grants(&queues, received);
            }
            endpoint.observed = endpoint.observed.max(observed);
            endpoint.grants_at = observed;
            endpoint.replace(queues, received);
        }
        Ok(())
    }
    pub(super) fn clear(&self) {
        let mut endpoints = self.endpoints.lock();
        // Release orders clear before every later insertion/readiness acknowledgement.
        self.cleared.fetch_add(1, Ordering::Release);
        let cleared_at = host_time();
        self.inventory_at.store(cleared_at, Ordering::Release);
        self.membership_at.fetch_max(cleared_at, Ordering::Release);
        let retired = std::mem::take(&mut *endpoints);
        drop(endpoints);
        drop(retired);
        // A preparer may currently hold the registry's control mutex inside a
        // storage call. It collects closed sources when its generation check fails.
        if let Some(_preparing) = self.preparing.try_lock() {
            self.sources.collect_retired();
        }
    }
}
fn validate_geometry(geometry: &[EndpointGeometry]) -> Result<(), Error> {
    if geometry.len() > 64
        || geometry.iter().enumerate().any(|(index, shape)| {
            shape.id == [0; 16]
                || shape.capture_channels > 64
                || shape.render_channels > 64
                || shape.capture_channels + shape.render_channels == 0
                || geometry[..index].iter().any(|other| other.id == shape.id)
        })
    {
        return Err(Error::Geometry);
    }
    Ok(())
}
fn log_grants(queues: &[ClientPcmReader], received: u64) {
    let native_ms = queues
        .iter()
        .find_map(ClientPcmReader::clock)
        .filter(|clock| clock.running)
        .map(|clock| {
            Timebase::current().nanoseconds(received.saturating_sub(clock.anchor)) / 1_000_000
        });
    tracing::info!(boundary = "hal_shared_pcm", stage = "grant_snapshot", clients = queues.len(), native_ms = ?native_ms, "virtual input grants received");
}

impl<W: Worker> Drop for VirtualPcm<W> {
    fn drop(&mut self) {
        self.clear();
    }
}
