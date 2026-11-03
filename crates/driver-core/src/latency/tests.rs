use super::*;
use crate::test_backend::FakeBackend;

#[test]
fn leases_fence_stale_publications_release_and_expiry() {
    let (mut backend, _) = FakeBackend::ready();
    let now = Instant::from_origin(Duration::ZERO);
    let mut authority = LatencyAuthority::new(Box::new(crate::test_runtime::FixedGenerations(100)));
    let claim = |name: &str| LatencyCommand::Claim {
        incarnation: name.into(),
        process_id: 10,
    };
    let first = authority
        .command(claim("first"), now, &mut backend)
        .unwrap()
        .lease
        .unwrap();
    assert!(
        !authority
            .command(claim("other"), now, &mut backend)
            .unwrap()
            .accepted
    );
    authority
        .expire(now.saturating_add(LEASE_TIMEOUT), &mut backend)
        .unwrap();
    let second = authority
        .command(
            claim("second"),
            now.saturating_add(LEASE_TIMEOUT),
            &mut backend,
        )
        .unwrap()
        .lease
        .unwrap();
    assert_ne!(first.generation, second.generation);
    assert!(
        !authority
            .command(
                LatencyCommand::Publish {
                    lease: first.clone(),
                    revision: 0,
                    destinations: vec![]
                },
                now.saturating_add(LEASE_TIMEOUT),
                &mut backend
            )
            .unwrap()
            .accepted
    );
    assert!(
        !authority
            .command(
                LatencyCommand::Release { lease: first },
                now.saturating_add(LEASE_TIMEOUT),
                &mut backend
            )
            .unwrap()
            .accepted
    );
    assert!(
        authority
            .command(
                LatencyCommand::Publish {
                    lease: second.clone(),
                    revision: 0,
                    destinations: vec![]
                },
                now.saturating_add(LEASE_TIMEOUT),
                &mut backend
            )
            .unwrap()
            .accepted
    );
    assert!(
        authority
            .command(
                LatencyCommand::Release { lease: second },
                now.saturating_add(LEASE_TIMEOUT),
                &mut backend
            )
            .unwrap()
            .accepted
    );
}

#[test]
fn runtime_codec_keeps_unknown_distinct_from_zero() {
    let lease = EngineLatencyLease {
        generation: 1,
        incarnation: "run".into(),
        process_id: 2,
    };
    let entry = RuntimeLatency {
        endpoint_id: VirtualEndpointId::new("device".into()),
        rate: 48_000,
        engine_lease: lease.clone(),
        applied_graph_revision: 3,
        input_latency_frames: Some(0),
    };
    assert_eq!(
        serde_json::from_value::<RuntimeLatency>(serde_json::to_value(&entry).unwrap()).unwrap(),
        entry
    );
    let unknown = RuntimeLatency {
        input_latency_frames: None,
        ..entry
    };
    assert_eq!(
        serde_json::from_str::<RuntimeLatency>(&serde_json::to_string(&unknown).unwrap()).unwrap(),
        unknown
    );
}

#[test]
fn stale_graphs_duplicates_and_expiry_keep_native_publication_correct() {
    let entry = example_publication();
    let (mut backend, observed) = FakeBackend::ready();
    let mut authority = LatencyAuthority::new(Box::new(crate::test_runtime::FixedGenerations(100)));
    let now = Instant::from_origin(Duration::ZERO);
    let lease = authority
        .command(
            LatencyCommand::Claim {
                incarnation: "run".into(),
                process_id: 2,
            },
            now,
            &mut backend,
        )
        .unwrap()
        .lease
        .unwrap();
    let entry = RuntimeLatency {
        engine_lease: lease.clone(),
        ..entry
    };
    authority
        .command(
            LatencyCommand::Publish {
                lease: lease.clone(),
                revision: 3,
                destinations: vec![entry.clone()],
            },
            now,
            &mut backend,
        )
        .unwrap();
    authority
        .command(
            LatencyCommand::Publish {
                lease: lease.clone(),
                revision: 2,
                destinations: vec![RuntimeLatency {
                    applied_graph_revision: 2,
                    input_latency_frames: None,
                    ..entry.clone()
                }],
            },
            now,
            &mut backend,
        )
        .unwrap();
    assert_eq!(authority.destinations[&entry.endpoint_id], entry);
    assert!(
        authority
            .command(
                LatencyCommand::Publish {
                    lease: lease.clone(),
                    revision: 3,
                    destinations: vec![entry.clone(), entry]
                },
                now,
                &mut backend
            )
            .is_err()
    );
    authority
        .expire(now.saturating_add(LEASE_TIMEOUT), &mut backend)
        .unwrap();
    assert!(authority.destinations.is_empty());
    assert_eq!(
        observed
            .lock()
            .latencies
            .last()
            .unwrap()
            .input_latency_frames,
        None
    );
}

/// A claimed lease whose clock moves only when a test advances `now`, so a slow test run
/// cannot expire the lease between publications.
struct Claimed {
    authority: LatencyAuthority,
    backend: FakeBackend,
    observed: std::sync::Arc<parking_lot::Mutex<crate::test_backend::Observation>>,
    lease: EngineLatencyLease,
    now: Instant,
}
impl Claimed {
    fn new() -> Self {
        let (mut backend, observed) = FakeBackend::ready();
        let mut authority =
            LatencyAuthority::new(Box::new(crate::test_runtime::FixedGenerations(100)));
        let now = Instant::from_origin(Duration::ZERO);
        let lease = authority
            .command(
                LatencyCommand::Claim {
                    incarnation: "run".into(),
                    process_id: 2,
                },
                now,
                &mut backend,
            )
            .unwrap()
            .lease
            .unwrap();
        Self {
            authority,
            backend,
            observed,
            lease,
            now,
        }
    }
    fn entry(&self, id: &str, revision: u64, frames: Option<u32>) -> RuntimeLatency {
        RuntimeLatency {
            endpoint_id: VirtualEndpointId::new(id.into()),
            engine_lease: self.lease.clone(),
            applied_graph_revision: revision,
            input_latency_frames: frames,
            ..example_publication()
        }
    }
    /// Publishes the complete set the engine applied at `revision`, one second later.
    fn publish(
        &mut self,
        revision: u64,
        destinations: Vec<RuntimeLatency>,
    ) -> Result<LatencyReply, DriverError> {
        self.now = self.now.saturating_add(Duration::from_secs(1));
        self.authority.command(
            LatencyCommand::Publish {
                lease: self.lease.clone(),
                revision,
                destinations,
            },
            self.now,
            &mut self.backend,
        )
    }
    /// The value the HAL last received for an endpoint.
    fn reported(&self, id: &str) -> Option<u32> {
        self.observed
            .lock()
            .latencies
            .iter()
            .rev()
            .find(|entry| entry.endpoint_id.as_str() == id)
            .and_then(|entry| entry.input_latency_frames)
    }
}

#[test]
fn an_endpoint_missing_from_the_next_publication_becomes_unknown() {
    let mut claimed = Claimed::new();
    let all = vec![
        claimed.entry("a", 10, Some(384)),
        claimed.entry("b", 10, Some(96)),
    ];
    claimed.publish(10, all).unwrap();
    assert_eq!(
        (claimed.reported("a"), claimed.reported("b")),
        (Some(384), Some(96))
    );

    // Removing the route to `a` leaves only `b` in the engine's complete set.
    let only_b = vec![claimed.entry("b", 11, Some(96))];
    claimed.publish(11, only_b.clone()).unwrap();
    assert_eq!(
        (claimed.reported("a"), claimed.reported("b")),
        (None, Some(96))
    );
    // Repeating that set sends nothing more for the already unknown endpoint.
    let count = claimed.observed.lock().latencies.len();
    claimed.publish(11, only_b).unwrap();
    assert_eq!(claimed.observed.lock().latencies.len(), count + 1);

    // The cleared endpoint keeps its revision fence: an older value cannot return.
    claimed
        .publish(9, vec![claimed.entry("a", 9, Some(512))])
        .unwrap();
    assert_eq!(claimed.reported("a"), None);
    claimed
        .publish(12, vec![claimed.entry("a", 12, Some(384))])
        .unwrap();
    assert_eq!(claimed.reported("a"), Some(384));
}

#[test]
fn a_failed_clear_is_retried_by_the_next_publication() {
    let mut claimed = Claimed::new();
    claimed
        .publish(
            10,
            vec![
                claimed.entry("a", 10, Some(384)),
                claimed.entry("b", 10, Some(96)),
            ],
        )
        .unwrap();
    // `b` fails after `a` was cleared; its measured entry stays so the next publication
    // retries it, and the already cleared `a` is not sent again.
    claimed.observed.lock().failed_latencies.insert("b".into());
    assert!(claimed.publish(10, vec![]).is_err());
    let b = VirtualEndpointId::new("b".into());
    assert_eq!(claimed.reported("a"), None);
    assert_eq!(
        claimed.authority.destinations[&b].input_latency_frames,
        Some(96)
    );
    claimed.observed.lock().failed_latencies.clear();
    let count = claimed.observed.lock().latencies.len();
    claimed.publish(10, vec![]).unwrap();
    assert_eq!(claimed.observed.lock().latencies.len(), count + 1);
    assert_eq!((claimed.reported("a"), claimed.reported("b")), (None, None));
    assert!(
        claimed
            .authority
            .destinations
            .values()
            .all(|entry| entry.input_latency_frames.is_none())
    );
}

/// The revision and frames the HAL last received for an endpoint.
fn last_sent(claimed: &Claimed, id: &str) -> Option<(u64, Option<u32>)> {
    claimed
        .observed
        .lock()
        .latencies
        .iter()
        .rev()
        .find(|entry| entry.endpoint_id.as_str() == id)
        .map(|entry| (entry.applied_graph_revision, entry.input_latency_frames))
}

#[test]
fn an_omitted_endpoint_is_fenced_at_the_omitting_publication_revision() {
    let mut claimed = Claimed::new();
    claimed
        .publish(
            10,
            vec![
                claimed.entry("a", 10, Some(384)),
                claimed.entry("b", 10, Some(96)),
            ],
        )
        .unwrap();
    claimed
        .publish(11, vec![claimed.entry("b", 11, Some(96))])
        .unwrap();
    // The HAL receives the clear at the omitting revision, so its own fence rises too.
    assert_eq!(last_sent(&claimed, "a"), Some((11, None)));
    // A delayed set from the revision that still listed `a` cannot measure it again.
    claimed
        .publish(
            10,
            vec![
                claimed.entry("a", 10, Some(512)),
                claimed.entry("b", 10, Some(96)),
            ],
        )
        .unwrap();
    assert_eq!(claimed.reported("a"), None);

    // `a` measured at 12, removed at 13, then a delayed set from 12 arrives.
    claimed
        .publish(
            12,
            vec![
                claimed.entry("a", 12, Some(384)),
                claimed.entry("b", 12, Some(96)),
            ],
        )
        .unwrap();
    claimed
        .publish(13, vec![claimed.entry("b", 13, Some(96))])
        .unwrap();
    claimed
        .publish(12, vec![claimed.entry("a", 12, Some(384))])
        .unwrap();
    assert_eq!(claimed.reported("a"), None);
    // The same revision may list the endpoint again once it reaches it.
    claimed
        .publish(13, vec![claimed.entry("a", 13, Some(384))])
        .unwrap();
    assert_eq!(claimed.reported("a"), Some(384));
}

#[test]
fn a_stale_complete_set_does_not_clear_a_newer_endpoint() {
    let mut claimed = Claimed::new();
    claimed
        .publish(10, vec![claimed.entry("a", 10, Some(384))])
        .unwrap();
    claimed
        .publish(12, vec![claimed.entry("b", 12, Some(96))])
        .unwrap();
    // The set from revision 10 arrives late; it neither omits `b` nor restores `a`.
    let count = claimed.observed.lock().latencies.len();
    let reply = claimed
        .publish(10, vec![claimed.entry("a", 10, Some(384))])
        .unwrap();
    assert!(reply.accepted);
    assert_eq!(claimed.observed.lock().latencies.len(), count);
    assert_eq!(
        (claimed.reported("a"), claimed.reported("b")),
        (None, Some(96))
    );
}

#[test]
fn an_empty_set_fences_omitted_endpoints_at_its_own_revision() {
    let mut claimed = Claimed::new();
    claimed
        .publish(10, vec![claimed.entry("a", 10, Some(384))])
        .unwrap();
    // Removing the last route publishes an empty set at the new graph revision.
    claimed.publish(12, vec![]).unwrap();
    assert_eq!(last_sent(&claimed, "a"), Some((12, None)));
    // A measurement from the revision in between arrives late.
    claimed
        .publish(11, vec![claimed.entry("a", 11, Some(512))])
        .unwrap();
    assert_eq!(claimed.reported("a"), None);
    claimed
        .publish(12, vec![claimed.entry("a", 12, Some(256))])
        .unwrap();
    assert_eq!(claimed.reported("a"), Some(256));
}

#[test]
fn a_failed_clear_keeps_the_omission_fence() {
    for later in [12, 13] {
        let mut claimed = Claimed::new();
        claimed
            .publish(10, vec![claimed.entry("a", 10, Some(384))])
            .unwrap();
        claimed.observed.lock().failed_latencies.insert("a".into());
        assert!(claimed.publish(12, vec![]).is_err());
        assert_eq!(last_sent(&claimed, "a"), Some((10, Some(384))));
        claimed.observed.lock().failed_latencies.clear();
        // A measurement from before the omission arrives in a later set; the omission at 12
        // still fences it, and the pending clear is sent instead.
        claimed
            .publish(later, vec![claimed.entry("a", 11, Some(512))])
            .unwrap();
        assert_eq!(last_sent(&claimed, "a"), Some((12, None)));
        claimed
            .publish(later, vec![claimed.entry("a", 12, Some(256))])
            .unwrap();
        assert_eq!(last_sent(&claimed, "a"), Some((12, Some(256))));
    }
}

#[test]
fn an_entry_newer_than_its_set_is_rejected_before_any_change() {
    let mut claimed = Claimed::new();
    claimed
        .publish(10, vec![claimed.entry("a", 10, Some(384))])
        .unwrap();
    let count = claimed.observed.lock().latencies.len();
    assert!(
        claimed
            .publish(11, vec![claimed.entry("b", 12, Some(96))])
            .is_err()
    );
    assert_eq!(claimed.observed.lock().latencies.len(), count);
    assert_eq!(claimed.reported("a"), Some(384));
}

#[test]
fn a_failing_first_endpoint_does_not_block_omitting_later_ones() {
    let mut claimed = Claimed::new();
    claimed
        .publish(
            10,
            vec![
                claimed.entry("a", 10, Some(384)),
                claimed.entry("b", 10, Some(96)),
            ],
        )
        .unwrap();
    claimed.observed.lock().failed_latencies.insert("a".into());
    assert!(claimed.publish(10, vec![]).is_err());
    let a = VirtualEndpointId::new("a".into());
    assert_eq!(
        (claimed.reported("a"), claimed.reported("b")),
        (Some(384), None)
    );
    assert_eq!(
        claimed.authority.destinations[&a].input_latency_frames,
        Some(384)
    );
    claimed.observed.lock().failed_latencies.clear();
    claimed.publish(10, vec![]).unwrap();
    assert_eq!(claimed.reported("a"), None);
}

#[test]
fn a_failing_first_endpoint_does_not_block_clearing_an_expired_lease() {
    let mut claimed = Claimed::new();
    claimed
        .publish(
            10,
            vec![
                claimed.entry("a", 10, Some(384)),
                claimed.entry("b", 10, Some(96)),
            ],
        )
        .unwrap();
    claimed.observed.lock().failed_latencies.insert("a".into());
    let expired = claimed.now.saturating_add(LEASE_TIMEOUT);
    assert!(
        claimed
            .authority
            .expire(expired, &mut claimed.backend)
            .is_err()
    );
    assert_eq!(
        (claimed.reported("a"), claimed.reported("b")),
        (Some(384), None)
    );
    // The failed endpoint and the lease stay, so the next expiry retries only `a`.
    let a = VirtualEndpointId::new("a".into());
    assert_eq!(
        claimed.authority.destinations.keys().collect::<Vec<_>>(),
        vec![&a]
    );
    let claim = LatencyCommand::Claim {
        incarnation: "next".into(),
        process_id: 3,
    };
    claimed.observed.lock().failed_latencies.clear();
    let count = claimed.observed.lock().latencies.len();
    let reply = claimed
        .authority
        .command(claim, expired, &mut claimed.backend)
        .unwrap();
    assert!(reply.accepted);
    assert_eq!(claimed.observed.lock().latencies.len(), count + 1);
    assert_eq!(claimed.reported("a"), None);
}

#[test]
fn invalid_identity_is_rejected_before_claiming_a_lease() {
    let (mut backend, _) = FakeBackend::ready();
    let mut authority = LatencyAuthority::new(Box::new(crate::test_runtime::FixedGenerations(100)));
    let now = Instant::from_origin(Duration::ZERO);
    assert!(
        authority
            .command(
                LatencyCommand::Claim {
                    incarnation: String::new(),
                    process_id: 0
                },
                now,
                &mut backend
            )
            .is_err()
    );
}

fn example_publication() -> RuntimeLatency {
    RuntimeLatency {
        endpoint_id: VirtualEndpointId::new("device".into()),
        rate: 48_000,
        engine_lease: EngineLatencyLease {
            generation: 1,
            incarnation: "run".into(),
            process_id: 2,
        },
        applied_graph_revision: 3,
        input_latency_frames: Some(0),
    }
}
