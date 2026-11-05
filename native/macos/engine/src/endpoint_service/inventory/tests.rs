//! Deferred inventory, cancellation, retry and reclamation use the actual control worker.
use super::*;
use mixrelay_macos_transport::client_pcm::ClientPcmSnapshot;
use mixrelay_macos_transport::{client_pcm::EndpointGeometry, source_registry::SourceRegistry};
use std::{
    cell::Cell,
    time::{Duration, Instant},
};

struct Clock;
impl mixrelay_time::MonotonicClock for Clock {
    fn now(&self) -> mixrelay_time::Instant {
        mixrelay_time::Instant::from_origin(Duration::ZERO)
    }
}
struct Manual(super::super::virtual_pcm::Pump);
impl Worker for Manual {
    fn start(pump: super::super::virtual_pcm::Pump) -> std::io::Result<Self> {
        Ok(Self(pump))
    }
    fn running(&self) -> bool {
        let _ = &self.0;
        true
    }
    fn wake(&self) {}
    fn finish(self) {}
}
// Each preparation worker has its own reservation failure; no test shares mutable state.
thread_local! { static FIRST: Cell<bool> = const { Cell::new(true) }; }
struct RetryOnce(Manual);
impl Worker for RetryOnce {
    fn start(pump: super::super::virtual_pcm::Pump) -> std::io::Result<Self> {
        if FIRST.replace(false) {
            return Err(std::io::ErrorKind::OutOfMemory.into());
        }
        Ok(Self(Manual(pump)))
    }
    fn running(&self) -> bool {
        self.0.running()
    }
    fn wake(&self) {}
    fn finish(self) {}
}
fn owner<W: Worker>() -> Arc<VirtualPcm<W>> {
    Arc::new(VirtualPcm::with_test_sources(SourceRegistry::with_storage(
        Box::new(Clock),
        mixrelay_realtime::Timebase::current(),
        super::super::virtual_pcm::test_storage::create,
    )))
}
fn snapshot(channels: Option<u32>) -> ClientPcmSnapshot {
    let mut snapshot = ClientPcmSnapshot::new().unwrap();
    if let Some(channels) = channels {
        assert!(snapshot.push_endpoint(EndpointGeometry {
            id: [1; 16],
            render_channels: channels,
            capture_channels: channels
        }));
    }
    snapshot
}
impl<W: Worker> Inventory<W> {
    fn enqueue(
        &self,
        pcm: Arc<VirtualPcm<W>>,
        snapshot: &ClientPcmSnapshot,
        received: u64,
    ) -> std::io::Result<()> {
        // SAFETY: the test owns this complete immutable HAL-shaped envelope.
        let (geometry, queues, observed) = unsafe {
            (
                ClientPcmSnapshot::endpoints(snapshot.as_raw()).unwrap(),
                ClientPcmSnapshot::decode(snapshot.as_raw()).unwrap(),
                ClientPcmSnapshot::observed_at(snapshot.as_raw()),
            )
        };
        let pending = pcm.replace_ready(queues, observed, received).unwrap();
        self.submit(
            pcm,
            Job {
                geometry,
                pending,
                observed,
                received,
            },
        )
    }
}
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "inventory worker did not complete"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
/// Waits for the worker to take every queued job, then stops it. Stopping joins the
/// worker after its in-flight preparation, so the registry reflects every submitted job.
fn finish<W: Worker>(inventory: &Inventory<W>) {
    wait_until(|| inventory.shared.state.lock().pending.is_none());
    inventory.stop();
}
/// Waits until a prepared virtual source is present in the registry.
fn wait_until_prepared<W: Worker>(pcm: &VirtualPcm<W>) {
    wait_until(|| {
        pcm.sources.set_consumer_route(
            token(),
            mixrelay_macos_transport::application_clock::ConsumerRoute::Unrouted,
        )
    });
}
#[test]
fn idle_geometry_is_prepared_off_the_grant_owner_and_latest_removal_reclaims_it() {
    let pcm = owner::<Manual>();
    let inventory = Inventory::new();
    inventory
        .enqueue(
            Arc::clone(&pcm),
            &snapshot(Some(2)),
            mixrelay_realtime::host_time(),
        )
        .unwrap();
    wait_until_prepared(&pcm);
    inventory
        .enqueue(
            Arc::clone(&pcm),
            &snapshot(Some(4)),
            mixrelay_realtime::host_time(),
        )
        .unwrap();
    inventory
        .enqueue(
            Arc::clone(&pcm),
            &snapshot(None),
            mixrelay_realtime::host_time(),
        )
        .unwrap();
    finish(&inventory);
    assert!(!pcm.sources.set_consumer_route(
        token(),
        mixrelay_macos_transport::application_clock::ConsumerRoute::Unrouted
    ));
    assert!(inventory.stopped());
}
#[test]
fn clear_discards_queued_or_in_flight_inventory_preparation() {
    let pcm = owner::<Manual>();
    let inventory = Inventory::new();
    inventory
        .enqueue(
            Arc::clone(&pcm),
            &snapshot(Some(2)),
            mixrelay_realtime::host_time(),
        )
        .unwrap();
    pcm.clear();
    finish(&inventory);
    assert!(!pcm.sources.set_consumer_route(
        token(),
        mixrelay_macos_transport::application_clock::ConsumerRoute::Unrouted
    ));
}
#[test]
fn failed_deferred_preparation_retries_without_another_hal_snapshot() {
    let pcm = owner::<RetryOnce>();
    let inventory = Inventory::new();
    inventory
        .enqueue(
            Arc::clone(&pcm),
            &snapshot(Some(2)),
            mixrelay_realtime::host_time(),
        )
        .unwrap();
    wait_until_prepared(&pcm);
}

fn token() -> u64 {
    b"virtual:01010101-0101-0101-0101-010101010101"
        .iter()
        .fold(14_695_981_039_346_656_037, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(1_099_511_628_211)
        })
}

#[test]
fn clear_generation_cancels_queued_work_independently_of_its_clock_stamp() {
    let pcm = owner::<Manual>();
    let inventory = Inventory::new();
    // A later stamp cannot authorize a job received before clear to resurrect storage.
    let observed = u64::MAX;
    let pending = pcm.replace_ready(Vec::new(), observed, 0).unwrap();
    pcm.clear();
    inventory
        .submit(
            Arc::clone(&pcm),
            Job {
                geometry: vec![EndpointGeometry {
                    id: [1; 16],
                    render_channels: 2,
                    capture_channels: 2,
                }],
                pending,
                observed,
                received: 0,
            },
        )
        .unwrap();
    finish(&inventory);
    assert!(!pcm.sources.set_consumer_route(
        token(),
        mixrelay_macos_transport::application_clock::ConsumerRoute::Unrouted
    ));
}
