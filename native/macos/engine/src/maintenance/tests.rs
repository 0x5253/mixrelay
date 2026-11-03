use super::*;
use std::sync::mpsc;

#[test]
fn dropping_worker_wakes_park_and_releases_its_captured_owner() {
    let owner = Arc::new(());
    let weak = Arc::downgrade(&owner);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let worker = MaintenanceWorker::start("maintenance-drop-test", move |stop| {
        ready_tx.send(()).unwrap();
        while !stop.load(Ordering::Acquire) {
            // A pending unpark token also covers drop racing with entry to park.
            thread::park();
        }
        drop(owner);
    })
    .unwrap();
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    drop(worker);

    assert!(weak.upgrade().is_none());
}

#[test]
fn stop_request_prevents_another_observation_before_join() {
    let (entered, observed) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let worker = MaintenanceWorker::start("observer-cancellation-test", move |stop| {
        while !stop.load(Ordering::Acquire) {
            count.fetch_add(1, Ordering::Relaxed);
            entered.send(()).unwrap();
            blocked.recv().unwrap();
        }
    })
    .unwrap();
    observed.recv_timeout(Duration::from_secs(2)).unwrap();
    worker.request_stop();
    release.send(()).unwrap();
    drop(worker);
    assert_eq!(calls.load(Ordering::Acquire), 1);
}

#[test]
fn dropping_worker_joins_an_in_flight_operation_before_returning() {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let (joined_tx, joined_rx) = mpsc::sync_channel(1);
    let finished = Arc::new(AtomicBool::new(false));
    let worker_finished = finished.clone();
    let worker = MaintenanceWorker::start("maintenance-join-test", move |_| {
        ready_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        worker_finished.store(true, Ordering::Release);
    })
    .unwrap();
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let stop = worker.stop.clone();
    let dropping = thread::spawn(move || {
        drop(worker);
        joined_tx.send(()).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while !stop.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline);
        thread::yield_now();
    }
    assert!(!finished.load(Ordering::Acquire));
    assert!(matches!(
        joined_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    release_tx.send(()).unwrap();
    joined_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    dropping.join().unwrap();
    assert!(finished.load(Ordering::Acquire));
}

#[test]
fn failed_later_startup_drops_previously_started_maintenance() {
    fn start_then_fail(owner: Arc<()>) -> Result<(), &'static str> {
        let _worker = MaintenanceWorker::start("maintenance-startup-test", move |stop| {
            while !stop.load(Ordering::Acquire) {
                thread::park();
            }
            drop(owner);
        })
        .unwrap();
        Err("later startup failed")
    }
    let owner = Arc::new(());
    let weak = Arc::downgrade(&owner);

    assert_eq!(start_then_fail(owner), Err("later startup failed"));

    assert!(weak.upgrade().is_none());
}

#[test]
fn reclamation_pass_releases_upgraded_exchange_before_waiting() {
    let exchange = Arc::new(ProcessorExchange::new(1));
    let weak = Arc::downgrade(&exchange);

    assert!(reclaim_once(&weak, &Weak::new()));

    assert_eq!(Arc::strong_count(&exchange), 1);
    drop(exchange);
    assert!(!reclaim_once(&weak, &Weak::new()));
    assert!(weak.upgrade().is_none());
}

#[test]
fn reclaimer_exits_when_the_engine_owner_is_gone() {
    let weak = Weak::new();
    let worker = start_reclaimer(weak.clone(), Weak::new()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !worker.thread.as_ref().unwrap().is_finished() {
        assert!(
            Instant::now() < deadline,
            "reclaimer retained its own exchange"
        );
        thread::sleep(Duration::from_millis(1));
    }

    assert!(weak.upgrade().is_none());
    drop(worker);
}

#[test]
fn explicit_reclaimer_finish_reports_a_panicked_worker_after_owner_release() {
    let owner = Arc::new(());
    let weak = Arc::downgrade(&owner);
    let worker = MaintenanceWorker::start("failed-reclaimer", move |_| {
        drop(owner);
        panic!("injected reclaim failure");
    })
    .unwrap();

    assert_eq!(
        worker.finish(),
        Err("failed-reclaimer panicked: injected reclaim failure".into())
    );
    assert!(weak.upgrade().is_none());
}
