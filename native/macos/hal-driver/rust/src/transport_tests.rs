use super::*;

fn client(key: Incarnation, bundle: Option<&str>) -> Client {
    Client {
        pid: key.0,
        incarnation: Some(key),
        bundle: bundle.map(str::to_owned),
    }
}

// Keep discovery queued until the test explicitly completes it. This makes
// timeout, queue pressure and refresh races deterministic without sleeping.
fn queued_transport(capacity: usize) -> (Transport, mpsc::Receiver<Query>) {
    let (queue, requests) = mpsc::sync_channel(capacity);
    let transport = Transport {
        state: Arc::new(Mutex::new(State::default())),
        queue,
        worker: None,
        inspection: ProcessInspection::SYSTEM,
        clock: Arc::new(mixrelay_time::StdClock::new()),
        coreaudio_pid: AtomicI32::new(0),
    };
    transport.policy(vec!["com.test.App".into()]);
    (transport, requests)
}
fn complete(transport: &Transport, query: &Query, bundle: Option<&str>) {
    let mut state = lock(&transport.state);
    if state
        .identities
        .get(&query.key)
        .is_some_and(|identity| Arc::ptr_eq(&identity.answer, &query.answer))
    {
        state.resolve(query.key, transport.clock.now(), bundle.map(str::to_owned));
    }
    let _ = query.done.try_send(());
}

#[test]
fn callback_identity_is_shared_across_devices_before_discovery() {
    let (transport, requests) = queued_transport(64);
    let pid = i32::try_from(std::process::id()).unwrap();
    let key = incarnation(pid).unwrap();
    transport.client(16, 1, pid, None);
    transport.client(32, 2, pid, Some("com.test.App".into()));
    assert_eq!(transport.get(16, pid), USB);
    assert_eq!(transport.get(48, pid), USB);
    assert!(requests.try_recv().is_err());
    transport.remove(32, 2);
    assert_eq!(transport.get(16, pid), USB);
    transport.remove(16, 1);
    assert!(!lock(&transport.state).identities.contains_key(&key));
    // A recycled PID cannot inherit callback identity or its USB decision.
    let reused = Incarnation(pid, key.1 + 1, key.2);
    assert_eq!(transport.query_key(reused, QUERY_BUDGET), VIRTUAL);
    complete(&transport, &requests.recv().unwrap(), None);
    assert_eq!(transport.query_key(reused, QUERY_BUDGET), VIRTUAL);
}

#[test]
fn zero_empty_policy_and_nested_audio_scopes_do_no_identity_work() {
    let transport =
        Transport::with_inspection(ProcessInspection::SYSTEM, mixrelay_time::StdClock::new());
    assert_eq!(transport.get(16, 0), VIRTUAL);
    assert_eq!(transport.get(16, i32::MAX), VIRTUAL);
    transport.policy(vec!["com.test.App".into()]);
    let pid = i32::try_from(std::process::id()).unwrap();
    transport.client(16, 1, pid, Some("com.test.App".into()));
    assert_eq!(transport.get(16, pid), USB);
    crate::abi::mr_hal_core_enter_audio_thread();
    {
        let _scope = AudioScope::enter();
        let _guard = lock(&transport.state);
        // Holding the resolver lock proves the IO fast path never takes it.
        assert_eq!(transport.get(16, pid), VIRTUAL);
    }
    assert_eq!(transport.get(16, pid), VIRTUAL);
    crate::abi::mr_hal_core_leave_audio_thread();
    assert_eq!(transport.get(16, pid), USB);
    transport.policy(vec![]);
    assert_eq!(transport.get(16, pid), VIRTUAL);
}

#[test]
fn completed_negative_decisions_expire_without_duplicate_refreshes() {
    let (transport, requests) = queued_transport(64);
    let key = Incarnation(123, 5, 9);
    assert_eq!(transport.query_key(key, QUERY_BUDGET), VIRTUAL);
    complete(&transport, &requests.recv().unwrap(), None);
    assert_eq!(transport.query_key(key, QUERY_BUDGET), VIRTUAL);
    assert!(requests.try_recv().is_err());
    lock(&transport.state)
        .identities
        .get_mut(&key)
        .unwrap()
        .expires = Instant::from_origin(Duration::ZERO);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..100 {
                    assert_eq!(transport.query_key(key, QUERY_BUDGET), VIRTUAL);
                }
            });
        }
    });
    let refresh = requests.recv().unwrap();
    assert!(requests.try_recv().is_err());
    let other = Incarnation(456, 8, 0);
    assert_eq!(transport.query_key(other, QUERY_BUDGET), VIRTUAL);
    let other_query = requests.recv().unwrap();
    // A completed negative remains stable, even if later discovery succeeds.
    complete(&transport, &refresh, Some("com.test.App"));
    assert_eq!(transport.query_key(key, QUERY_BUDGET), VIRTUAL);
    complete(&transport, &other_query, Some("com.test.App"));
    assert_eq!(transport.query_key(other, QUERY_BUDGET), USB);
    assert!(!lock(&transport.state).identities[&key].in_flight);
    assert!(transport.policy(vec!["com.test.Other".into()]));
    assert!(!transport.policy(vec!["com.test.Other".into()]));
    assert!(lock(&transport.state).identities.is_empty());
}

#[test]
fn full_queue_and_slow_lookup_are_transient_and_allow_retry() {
    let (transport, requests) = queued_transport(64);
    let blocker = Incarnation(1, 2, 3);
    // This request times out while the lookup remains outstanding.
    assert_eq!(transport.query_key(blocker, QUERY_BUDGET), VIRTUAL);
    let slow = requests.recv().unwrap();
    for pid in 2..66 {
        assert_eq!(
            transport.query_key(Incarnation(pid, 2, 3), QUERY_BUDGET),
            VIRTUAL
        );
    }
    let denied = Incarnation(66, 2, 3);
    assert_eq!(transport.query_key(denied, QUERY_BUDGET), VIRTUAL);
    {
        let state = lock(&transport.state);
        assert_eq!(
            state.identities[&blocker].answer.load(Ordering::Acquire),
            PENDING
        );
        assert!(state.identities[&blocker].in_flight);
        assert_eq!(
            state.identities[&denied].answer.load(Ordering::Acquire),
            PENDING
        );
        assert!(!state.identities[&denied].in_flight);
    }
    // The slow requester can still become USB after its caller has timed out.
    complete(&transport, &slow, Some("com.test.App"));
    assert_eq!(transport.query_key(blocker, QUERY_BUDGET), USB);
    // Drain queued work, making room for the requester previously denied admission.
    for _ in 0..64 {
        complete(&transport, &requests.recv().unwrap(), None);
    }
    assert_eq!(transport.query_key(denied, QUERY_BUDGET), VIRTUAL);
    complete(&transport, &requests.recv().unwrap(), Some("com.test.App"));
    assert_eq!(transport.query_key(denied, QUERY_BUDGET), USB);
}

#[test]
fn worker_completion_after_timeout_can_resolve_other_queued_requesters() {
    let (entered, lookups) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let transport = Transport::with_resolvers(
        move |key| {
            entered.send(key).unwrap();
            // Test failure must not strand the worker during Transport::drop.
            blocked.recv_timeout(Duration::from_secs(2)).ok()?;
            Some("com.test.App".into())
        },
        |pid| Some(Incarnation(pid, 4, 0)),
        mixrelay_time::StdClock::new(),
    );
    transport.policy(vec!["com.test.App".into()]);
    let slow = Incarnation(123, 4, 0);
    let other = Incarnation(456, 4, 0);
    assert_eq!(transport.query_key(slow, QUERY_BUDGET), VIRTUAL);
    assert!(lookups.recv_timeout(Duration::from_secs(1)).unwrap() == slow);
    for _ in 0..100 {
        assert_eq!(transport.query_key(other, QUERY_BUDGET), VIRTUAL);
    }
    assert_eq!(
        lock(&transport.state).identities[&other]
            .answer
            .load(Ordering::Acquire),
        PENDING
    );
    release.send(()).unwrap();
    // Entering the next lookup proves the slow request was published first.
    assert!(lookups.recv_timeout(Duration::from_secs(1)).unwrap() == other);
    assert_eq!(transport.query_key(slow, QUERY_BUDGET), USB);
    release.send(()).unwrap();
    drop(release);
    let state = Arc::clone(&transport.state);
    drop(transport); // joins the real worker, including the second publication
    assert_eq!(
        lock(&state).identities[&other]
            .answer
            .load(Ordering::Acquire),
        USB
    );
    assert!(!lock(&state).identities[&other].in_flight);
    assert!(lookups.try_recv().is_err()); // no duplicate worker lookups
}

#[test]
fn churn_reclaims_last_client_and_recycled_pid_decisions() {
    let (transport, requests) = queued_transport(64);
    for start in 0..(MAX_IDENTITIES * 2) {
        let key = Incarnation(123, u64::try_from(start).unwrap(), 0);
        lock(&transport.state).client(16, 1, client(key, Some("com.test.App")));
        assert_eq!(transport.query_key(key, QUERY_BUDGET), USB);
        // Another device still owns a registration for this process.
        lock(&transport.state).client(32, 2, client(key, None));
        transport.remove(16, 1);
        assert_eq!(transport.query_key(key, QUERY_BUDGET), USB);
        transport.remove(32, 2);
        assert!(lock(&transport.state).identities.is_empty());
    }
    // Query-only requesters also release the predecessor on PID reuse.
    let old = Incarnation(123, 1, 0);
    transport.query_key(old, QUERY_BUDGET);
    let retired = requests.recv().unwrap();
    let new = Incarnation(123, 2, 0);
    lock(&transport.state).client(16, 1, client(new, Some("com.test.App")));
    assert_eq!(transport.query_key(new, QUERY_BUDGET), USB);
    complete(&transport, &retired, None);
    let state = lock(&transport.state);
    assert_eq!(state.identities.len(), 1);
    assert!(!state.identities.contains_key(&old));
}

fn fill_capacity(transport: &Transport) {
    let mut state = lock(&transport.state);
    for index in 1..=MAX_IDENTITIES {
        state.identities.insert(
            Incarnation(-i32::try_from(index).unwrap(), 1, 0),
            Identity {
                bundle: None,
                expires: Instant::from_origin(Duration::ZERO),
                answer: Arc::new(AtomicU32::new(if index % 2 == 0 { USB } else { VIRTUAL })),
                in_flight: false,
            },
        );
    }
}

#[test]
fn full_capacity_misses_are_transient_without_inline_reclamation() {
    let (transport, requests) = queued_transport(64);
    fill_capacity(&transport);
    let key = Incarnation(123, 5, 9);
    lock(&transport.state).client(32, 1, client(key, Some("com.test.App")));
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..100 {
                    assert_eq!(transport.query_key(key, QUERY_BUDGET), VIRTUAL);
                }
            });
        }
    });
    assert_eq!(lock(&transport.state).identities.len(), MAX_IDENTITIES);
    assert!(!lock(&transport.state).identities.contains_key(&key));
    assert!(requests.try_recv().is_err());
    let mut cursor = None;
    let mut inspected = 0;
    reclaim_step(&transport.state, &mut cursor, |_| {
        assert!(transport.state.try_lock().is_ok()); // no lock during proc_pidinfo
        inspected += 1;
        None
    });
    assert_eq!(inspected, RECLAIM_STEP);
    assert_eq!(
        lock(&transport.state).identities.len(),
        MAX_IDENTITIES - RECLAIM_STEP
    );
    // The refusal did not cache a negative or lose the callback identity.
    assert_eq!(transport.query_key(key, QUERY_BUDGET), USB);
    while cursor.is_some() {
        reclaim_step(&transport.state, &mut cursor, |pid| {
            (pid == key.0).then_some(key)
        });
    }
    assert_eq!(lock(&transport.state).identities.len(), 1);
}

#[test]
fn background_worker_reclaims_without_property_misses() {
    let (inspected, steps) = mpsc::channel();
    let (release, permits) = mpsc::channel();
    let transport = Transport::with_resolvers(
        |_| None,
        move |pid| {
            inspected.send(pid).unwrap();
            permits.recv_timeout(Duration::from_secs(2)).ok()?;
            None
        },
        mixrelay_time::StdClock::new(),
    );
    fill_capacity(&transport);
    for remaining in (MAX_IDENTITIES - RECLAIM_STEP..MAX_IDENTITIES).rev() {
        assert!(steps.recv_timeout(Duration::from_secs(1)).unwrap() < 0);
        // Only this worker performs cleanup, one process inspection at a time.
        assert!(steps.try_recv().is_err());
        assert_eq!(lock(&transport.state).identities.len(), remaining + 1);
        release.send(()).unwrap();
    }
    // The next inspection proves the first bounded step has finished.
    steps.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(
        lock(&transport.state).identities.len(),
        MAX_IDENTITIES - RECLAIM_STEP
    );
    drop(release);
    drop(transport);
}

#[test]
fn reclamation_preserves_live_and_replaced_entries() {
    let (transport, _) = queued_transport(64);
    let key = Incarnation(123, 5, 9);
    lock(&transport.state).client(16, 1, client(key, Some("com.test.App")));
    assert_eq!(transport.query_key(key, QUERY_BUDGET), USB);
    let mut cursor = None;
    reclaim_step(&transport.state, &mut cursor, |_| Some(key));
    assert_eq!(transport.query_key(key, QUERY_BUDGET), USB);
    let old = Arc::clone(&lock(&transport.state).identities[&key].answer);
    reclaim_step(&transport.state, &mut cursor, |_| {
        transport.policy(vec!["com.test.Other".into()]);
        transport.query_key(key, QUERY_BUDGET);
        None
    });
    let state = lock(&transport.state);
    assert!(!Arc::ptr_eq(&old, &state.identities[&key].answer));
    assert_eq!(
        state.identities[&key].answer.load(Ordering::Acquire),
        VIRTUAL
    );
}

/// Logical time: identity work advances it explicitly and every discovery wait is recorded,
/// so deadline arithmetic is observed exactly instead of through elapsed wall time.
struct LogicalClock {
    now: Mutex<Instant>,
    waits: Mutex<Vec<Duration>>,
}
impl LogicalClock {
    fn new() -> Self {
        Self {
            now: Mutex::new(Instant::from_origin(Duration::ZERO)),
            waits: Mutex::default(),
        }
    }
    fn advance(&self, elapsed: Duration) {
        let mut now = lock(&self.now);
        *now = now.saturating_add(elapsed);
    }
}
impl MonotonicClock for LogicalClock {
    fn now(&self) -> Instant {
        *lock(&self.now)
    }
}
impl Clock for LogicalClock {
    fn wait(&self, _completion: &mpsc::Receiver<()>, timeout: Duration) {
        // Discovery stays queued, so the wait runs to its timeout.
        lock(&self.waits).push(timeout);
        self.advance(timeout);
    }
}

#[test]
fn property_deadline_covers_identity_work_and_remaining_completion_wait() {
    let (transport, requests) = queued_transport(64);
    let clock = LogicalClock::new();
    let key = Incarnation(123, 5, 9);
    // Identity work spends 150 ms of a 200 ms budget, so the completion wait gets only
    // the remaining 50 ms, never a fresh full budget.
    let deadline = clock.now().saturating_add(Duration::from_millis(200));
    let identity = |_| {
        clock.advance(Duration::from_millis(150));
        Some(key)
    };
    assert_eq!(
        transport.get_with_clock(key.0, deadline, identity, &clock),
        VIRTUAL
    );
    assert_eq!(*lock(&clock.waits), [Duration::from_millis(50)]);
    assert_eq!(clock.now(), deadline);
    let query = requests.try_recv().unwrap();
    assert_eq!(query.answer.load(Ordering::Acquire), PENDING);
    // Identity work that exhausts the actual 20 ms budget must not enqueue or wait.
    let other = Incarnation(456, 5, 9);
    let exhausted = |_| {
        clock.advance(QUERY_BUDGET * 2);
        Some(other)
    };
    assert_eq!(
        transport.get_with_clock(
            other.0,
            clock.now().saturating_add(QUERY_BUDGET),
            exhausted,
            &clock
        ),
        VIRTUAL
    );
    assert!(!lock(&transport.state).identities.contains_key(&other));
    assert!(requests.try_recv().is_err());
    assert_eq!(lock(&clock.waits).len(), 1);
    complete(&transport, &query, Some("com.test.App"));
    assert_eq!(transport.query_key(key, QUERY_BUDGET), USB);
}

#[test]
fn contended_resolver_lock_returns_transiently_without_waiting() {
    let (transport, requests) = queued_transport(64);
    let pid = i32::try_from(std::process::id()).unwrap();
    let key = incarnation(pid).unwrap();
    lock(&transport.state).client(16, 1, client(key, Some("com.test.App")));
    assert_eq!(transport.get(16, pid), USB);
    let (done, finished) = mpsc::channel();
    let timely = std::thread::scope(|scope| {
        let guard = lock(&transport.state);
        scope.spawn(|| {
            done.send((
                transport.get(16, pid),
                transport.query_key(key, QUERY_BUDGET),
            ))
            .unwrap();
        });
        let result = finished.recv_timeout(Duration::from_millis(200));
        drop(guard); // regression failure cannot strand the test thread
        result
    });
    assert_eq!(timely.unwrap(), (VIRTUAL, VIRTUAL));
    assert!(requests.try_recv().is_err());
    assert_eq!(transport.get(16, pid), USB);
}

#[test]
fn policy_replacement_and_client_removal_retire_pending_discovery() {
    let (transport, requests) = queued_transport(64);
    let key = Incarnation(123, 4, 0);
    lock(&transport.state).client(16, 1, client(key, None));
    transport.query_key(key, QUERY_BUDGET);
    let removed = requests.recv().unwrap();
    transport.remove(16, 1);
    complete(&transport, &removed, Some("com.test.App"));
    assert!(lock(&transport.state).identities.is_empty());
    transport.query_key(key, QUERY_BUDGET);
    let old_policy = requests.recv().unwrap();
    transport.policy(vec!["com.test.Other".into()]);
    transport.query_key(key, QUERY_BUDGET);
    let current = requests.recv().unwrap();
    complete(&transport, &old_policy, Some("com.test.App"));
    assert_eq!(
        lock(&transport.state).identities[&key]
            .answer
            .load(Ordering::Acquire),
        PENDING
    );
    complete(&transport, &current, Some("com.test.Other"));
    assert_eq!(transport.query_key(key, QUERY_BUDGET), USB);
}

#[test]
fn unknown_process_worker_caches_misses() {
    let transport =
        Transport::with_inspection(ProcessInspection::SYSTEM, mixrelay_time::StdClock::new());
    let pid = i32::try_from(std::process::id()).unwrap();
    transport.policy(vec!["com.test.NoMatch".into()]);
    assert_eq!(transport.query(16, pid, Duration::from_secs(1)), VIRTUAL);
    let key = incarnation(pid).unwrap();
    let state = lock(&transport.state);
    assert_eq!(
        state.identities[&key].answer.load(Ordering::Acquire),
        VIRTUAL
    );
    assert!(!state.identities[&key].in_flight);
    assert!(discover(Incarnation(pid, key.1 + 1, key.2)).is_none());
}

#[test]
fn the_enclosing_app_identifies_an_executable_without_launching_it() {
    unsafe extern "C" {
        fn mr_hal_identity_bundle_at(
            executable: *const c_char,
            output: *mut c_char,
            capacity: usize,
        ) -> bool;
    }
    let identify = |path: &std::path::Path| {
        let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        let mut bytes = [0 as c_char; 128];
        unsafe { mr_hal_identity_bundle_at(path.as_ptr(), bytes.as_mut_ptr(), bytes.len()) }.then(
            || {
                unsafe { CStr::from_ptr(bytes.as_ptr()) }
                    .to_string_lossy()
                    .into_owned()
            },
        )
    };
    // A bundle layout with a placeholder file: Info.plist is read, nothing is executed,
    // so macOS never assesses (or reports as damaged) an unsigned test application.
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../target/test-artifacts")
        .join(format!("transport-bundle-{}", std::process::id()));
    let contents = root.join("Fixture.app/Contents");
    std::fs::create_dir_all(contents.join("MacOS/Helpers")).unwrap();
    std::fs::write(contents.join("MacOS/fixture"), b"").unwrap();
    std::fs::write(contents.join("Info.plist"), r#"<?xml version="1.0"?>
<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.test.BundleProbe</string></dict></plist>"#).unwrap();
    let found = identify(&contents.join("MacOS/fixture"));
    let nested = identify(&contents.join("MacOS/Helpers/tool"));
    let outside = identify(&root.join("tool"));
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(found.as_deref(), Some("com.test.BundleProbe"));
    assert_eq!(nested.as_deref(), Some("com.test.BundleProbe"));
    assert_eq!(outside, None);
    // This unbundled test process has no enclosing application.
    let pid = i32::try_from(std::process::id()).unwrap();
    assert_eq!(bundle(pid), None);
}

#[test]
fn unknown_bundle_is_resolved_by_the_worker_and_answered_within_the_caller_budget() {
    let pid = i32::try_from(std::process::id()).unwrap();
    let transport = Transport::with_discovery(move |key| {
        (key.0 == pid).then(|| "com.test.BundleProbe".to_owned())
    });
    transport.policy(vec!["com.test.BundleProbe".into()]);
    // Inject a generous caller budget; production remains bounded to 20 ms.
    assert_eq!(transport.query(16, pid, Duration::from_secs(1)), USB);
    assert_eq!(transport.query(16, pid, QUERY_BUDGET), USB);
}

#[test]
fn a_registered_client_answers_from_its_callback_bundle_without_process_inspection() {
    let (transport, requests) = queued_transport(64);
    // The driver sandbox refuses inspection: no incarnation for any PID.
    let refused = |_| None;
    transport.client(16, 1, 4242, Some("com.test.App".into()));
    transport.client(16, 2, 4343, Some("com.other.App".into()));
    transport.client(16, 3, 4444, None);
    let deadline = transport.deadline(QUERY_BUDGET);
    assert_eq!(transport.get_with_incarnation(4242, deadline, refused), USB);
    assert_eq!(
        transport.get_with_incarnation(4343, deadline, refused),
        VIRTUAL
    );
    // No bundle from the callback and no inspection: the safe default.
    assert_eq!(
        transport.get_with_incarnation(4444, deadline, refused),
        VIRTUAL
    );
    assert!(
        requests.try_recv().is_err(),
        "registered clients need no discovery work"
    );
    // RemoveDeviceClient ends the registration.
    transport.remove(16, 1);
    assert_eq!(
        transport.get_with_incarnation(4242, deadline, refused),
        VIRTUAL
    );
    // An empty policy answers virt even for a registered client.
    transport.client(16, 1, 4242, Some("com.test.App".into()));
    transport.policy(Vec::new());
    assert_eq!(
        transport.get_with_incarnation(4242, deadline, refused),
        VIRTUAL
    );
}

#[test]
fn a_reused_pid_does_not_inherit_its_predecessors_registered_bundle() {
    let (transport, requests) = queued_transport(64);
    let old = Incarnation(4242, 1, 0);
    let new = Incarnation(4242, 2, 0);
    let read = |key: Incarnation| {
        transport.get_with_incarnation(key.0, transport.deadline(QUERY_BUDGET), |_| Some(key))
    };
    lock(&transport.state).client(16, 1, client(old, Some("com.test.App")));
    assert_eq!(read(old), USB);
    // The new process registers with no bundle while the old registration lingers.
    lock(&transport.state).client(32, 2, client(new, None));
    assert_eq!(read(new), VIRTUAL);
    complete(&transport, &requests.recv().unwrap(), None);
    assert_eq!(read(new), VIRTUAL);
    // The old registration's late removal leaves the new process's own answer.
    transport.remove(16, 1);
    lock(&transport.state).client(48, 3, client(new, Some("com.test.App")));
    assert_eq!(read(new), USB);
    assert!(requests.try_recv().is_err());
    // A replaced registration without a bundle and an unknown incarnation keeps it.
    lock(&transport.state).client(
        48,
        3,
        Client {
            pid: new.0,
            incarnation: None,
            bundle: None,
        },
    );
    assert_eq!(read(new), USB);
    transport.remove(48, 3);
    transport.remove(32, 2);
    assert!(lock(&transport.state).bundles.is_empty());
}

#[test]
fn a_reused_pid_querying_before_it_registers_does_not_receive_the_predecessors_answer() {
    let (transport, requests) = queued_transport(64);
    let old = Incarnation(4242, 1, 0);
    let new = Incarnation(4242, 2, 0);
    let read = |pid: i32, current: Option<Incarnation>| {
        transport.get_with_incarnation(pid, transport.deadline(QUERY_BUDGET), |_| current)
    };
    lock(&transport.state).client(16, 1, client(old, Some("com.test.App")));
    // Denied inspection keeps the registered answer, as the driver sandbox requires.
    assert_eq!(read(old.0, None), USB);
    assert_eq!(read(old.0, Some(old)), USB);
    // The replacement is identified by inspection and never inherits USB.
    assert_eq!(read(new.0, Some(new)), VIRTUAL);
    complete(&transport, &requests.recv().unwrap(), None);
    assert_eq!(read(new.0, Some(new)), VIRTUAL);
    // The stale bundle is retired, so denied inspection no longer reports it either.
    assert!(lock(&transport.state).bundles.is_empty());
    assert_eq!(read(new.0, None), VIRTUAL);
}

#[test]
fn a_stale_bundle_stays_retired_when_the_mismatching_read_meets_contention() {
    let (transport, _requests) = queued_transport(64);
    let old = Incarnation(4242, 1, 0);
    let new = Incarnation(4242, 2, 0);
    lock(&transport.state).client(16, 1, client(old, Some("com.test.App")));
    // Another thread takes the state lock between the registered read and
    // retirement, so neither retirement nor discovery can lock.
    let (held, holding) = mpsc::sync_channel(0);
    let (release, released) = mpsc::sync_channel::<()>(0);
    std::thread::scope(|scope| {
        let answer =
            transport.get_with_incarnation(new.0, transport.deadline(QUERY_BUDGET), |_| {
                let state = &transport.state;
                scope.spawn(move || {
                    let _guard = lock(state);
                    held.send(()).unwrap();
                    released.recv().unwrap();
                });
                holding.recv().unwrap();
                Some(new)
            });
        assert_eq!(answer, VIRTUAL);
        release.send(()).unwrap();
    });
    assert_eq!(
        transport.get_with_incarnation(new.0, transport.deadline(QUERY_BUDGET), |_| None),
        VIRTUAL
    );
}

#[test]
fn a_registered_answer_respects_the_caller_deadline_after_inspection() {
    let (transport, _requests) = queued_transport(64);
    let key = Incarnation(4242, 1, 0);
    lock(&transport.state).client(16, 1, client(key, Some("com.test.App")));
    let slow = |current: Option<Incarnation>| {
        let clock = LogicalClock::new();
        let deadline = clock.now().saturating_add(Duration::from_millis(5));
        let inspection = |_| {
            clock.advance(Duration::from_millis(10));
            current
        };
        transport.get_with_clock(key.0, deadline, inspection, &clock)
    };
    assert_eq!(slow(Some(key)), VIRTUAL);
    assert_eq!(slow(None), VIRTUAL);
    // The timeout is transient: the registration still answers within budget.
    assert_eq!(
        transport.get_with_incarnation(key.0, transport.deadline(QUERY_BUDGET), |_| Some(key)),
        USB
    );
}
