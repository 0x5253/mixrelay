use std::{sync::Arc, time::Duration};

use mixrelay_time::{Instant, ManualClock};
use parking_lot::Mutex;

use super::*;
use crate::{
    DriverManager, EngineLatencyLease, LatencyCommand, RuntimeLatency,
    test_backend::{FakeBackend, Observation},
    test_settings::MemorySettings,
};

#[derive(Clone)]
struct Generations(Arc<Mutex<GenerationObservation>>);

struct GenerationObservation {
    candidate: u64,
    reads: usize,
}

impl LatencyGenerations for Generations {
    fn candidate(&self) -> u64 {
        let mut observation = self.0.lock();
        observation.reads += 1;
        observation.candidate
    }
}

struct Fixture {
    driver: DriverManager,
    clock: ManualClock,
    generations: Generations,
    native: Arc<Mutex<Observation>>,
}

impl Fixture {
    fn new() -> Self {
        let clock = ManualClock::new(Instant::from_origin(Duration::from_secs(123)));
        let generations = Generations(Arc::new(Mutex::new(GenerationObservation {
            candidate: 100,
            reads: 0,
        })));
        let runtime = DriverRuntime::new(
            DriverProcessId::new(77).unwrap(),
            Box::new(clock.clone()),
            Box::new(generations.clone()),
        );
        let (backend, native) = FakeBackend::ready();
        let driver = DriverManager::open(backend, MemorySettings::default(), runtime).unwrap();
        Self {
            driver,
            clock,
            generations,
            native,
        }
    }

    fn claim(&mut self, incarnation: &str) -> EngineLatencyLease {
        self.driver
            .publish_latency(claim(incarnation))
            .unwrap()
            .lease
            .unwrap()
    }
}

fn claim(incarnation: &str) -> LatencyCommand {
    LatencyCommand::Claim {
        incarnation: incarnation.into(),
        process_id: 10,
    }
}

#[test]
fn owner_identity_and_exact_lease_expiry_use_supplied_runtime_inputs() {
    let mut fixture = Fixture::new();
    let snapshot = fixture.driver.snapshot().unwrap();
    assert_eq!(snapshot.owner_process_id, Some(77));
    let lease = fixture.claim("engine");
    fixture
        .driver
        .publish_latency(LatencyCommand::Publish {
            lease: lease.clone(),
            revision: 1,
            destinations: vec![RuntimeLatency {
                endpoint_id: snapshot.settings.devices[0].spec.id.clone(),
                rate: 48_000,
                engine_lease: lease,
                applied_graph_revision: 1,
                input_latency_frames: Some(512),
            }],
        })
        .unwrap();
    fixture
        .clock
        .advance(Duration::from_secs(3) - Duration::from_nanos(1));
    fixture.driver.maintain().recovery.unwrap();
    assert_eq!(
        fixture
            .native
            .lock()
            .latencies
            .last()
            .unwrap()
            .input_latency_frames,
        Some(512)
    );
    fixture.clock.advance(Duration::from_nanos(1));
    fixture.driver.maintain().recovery.unwrap();
    assert_eq!(
        fixture
            .native
            .lock()
            .latencies
            .last()
            .unwrap()
            .input_latency_frames,
        None
    );
    assert_eq!(
        fixture.driver.snapshot().unwrap().revision,
        snapshot.revision
    );
}

#[test]
fn only_accepted_claims_sample_generations_and_older_candidates_still_advance() {
    let mut fixture = Fixture::new();
    assert!(fixture.driver.publish_latency(claim("")).is_err());
    assert_eq!(fixture.generations.0.lock().reads, 0);
    let first = fixture.claim("first");
    assert_eq!(first.generation, 100);
    assert!(
        !fixture
            .driver
            .publish_latency(claim("busy"))
            .unwrap()
            .accepted
    );
    assert_eq!(fixture.generations.0.lock().reads, 1);
    fixture
        .driver
        .publish_latency(LatencyCommand::Release { lease: first })
        .unwrap();
    fixture.generations.0.lock().candidate = 0;
    assert_eq!(fixture.claim("next").generation, 101);
    assert_eq!(fixture.generations.0.lock().reads, 2);
}

#[test]
fn generation_exhaustion_rejects_a_claim_before_sampling_or_publishing() {
    let mut fixture = Fixture::new();
    fixture.generations.0.lock().candidate = u64::MAX;
    let lease = fixture.claim("last");
    assert_eq!(lease.generation, u64::MAX);
    fixture
        .driver
        .publish_latency(LatencyCommand::Release { lease })
        .unwrap();
    assert!(matches!(
        fixture.driver.publish_latency(claim("exhausted")),
        Err(DriverError::Unavailable(_))
    ));
    assert_eq!(fixture.generations.0.lock().reads, 1);
    assert!(fixture.native.lock().latencies.is_empty());
}

#[test]
fn driver_process_identity_rejects_zero_and_out_of_range_native_values() {
    assert!(DriverProcessId::new(0).is_err());
    assert_eq!(DriverProcessId::new(1).unwrap().get(), 1);
    assert_eq!(
        DriverProcessId::new(i32::MAX.cast_unsigned())
            .unwrap()
            .get(),
        i32::MAX.cast_unsigned()
    );
    assert!(DriverProcessId::new(i32::MAX.cast_unsigned() + 1).is_err());
}

#[test]
fn failed_latency_clear_remains_observable_while_inventory_recovers() {
    let mut fixture = Fixture::new();
    let endpoint = fixture.driver.snapshot().unwrap().settings.devices[0]
        .spec
        .id
        .clone();
    let lease = fixture.claim("engine");
    fixture
        .driver
        .publish_latency(LatencyCommand::Publish {
            lease: lease.clone(),
            revision: 1,
            destinations: vec![RuntimeLatency {
                endpoint_id: endpoint.clone(),
                rate: 48_000,
                engine_lease: lease,
                applied_graph_revision: 1,
                input_latency_frames: Some(512),
            }],
        })
        .unwrap();
    {
        let mut native = fixture.native.lock();
        native.failed_latencies.insert(endpoint.as_str().into());
        native.failed_devices.insert(endpoint.as_str().into());
    }
    fixture.clock.advance(Duration::from_secs(3));
    let result = fixture.driver.maintain();
    assert!(matches!(result.latency, Err(DriverError::Backend(_))));
    assert!(result.recovery.unwrap());
    assert_eq!(fixture.native.lock().recovered_devices, [endpoint.as_str()]);
    fixture.native.lock().failed_latencies.clear();
    let result = fixture.driver.maintain();
    result.latency.unwrap();
    assert!(!result.recovery.unwrap());
    assert_eq!(
        fixture
            .native
            .lock()
            .latencies
            .last()
            .unwrap()
            .input_latency_frames,
        None
    );
}
