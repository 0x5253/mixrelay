use super::*;
use mixrelay_time::{Instant, ManualClock, MonotonicClock};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Timer(ManualClock);
impl MonotonicClock for Timer {
    fn now(&self) -> Instant {
        self.0.now()
    }
}
impl AsyncDelay for Timer {
    async fn wait_until(&self, deadline: Deadline) {
        self.0.advance(deadline.remaining(self.0.now()));
    }
}
struct Endpoint {
    available: Arc<AtomicBool>,
    ready: bool,
    pending: bool,
    protocol: u64,
}
impl RuntimeEndpoint for Endpoint {
    async fn hello(&self) -> Option<Value> {
        if self.pending {
            return std::future::pending().await;
        }
        self.available.load(Ordering::Relaxed).then(|| {
            serde_json::json!({
                "protocolVersion":self.protocol,"product":"mixrelay-driver","version":"1.0.0"
            })
        })
    }
    async fn engine_ready(&self) -> bool {
        self.ready
    }
}
struct Launch {
    exited: bool,
}
impl LaunchObservation for Launch {
    fn exited(&self) -> bool {
        self.exited
    }
}
struct Launcher {
    admitted: bool,
    launches: AtomicUsize,
    available: Arc<AtomicBool>,
    ready_after: usize,
    exited: bool,
}
impl ControllerLauncher for Launcher {
    type Launch = Launch;
    fn admission_available(&self) -> std::io::Result<bool> {
        Ok(self.admitted)
    }
    fn launch(&self) -> std::io::Result<Launch> {
        let count = self.launches.fetch_add(1, Ordering::Relaxed) + 1;
        if count == self.ready_after {
            self.available.store(true, Ordering::Relaxed);
        }
        Ok(Launch {
            exited: self.exited,
        })
    }
}
fn setup(admitted: bool, ready_after: usize, exited: bool) -> (Endpoint, Launcher, Timer) {
    let available = Arc::new(AtomicBool::new(false));
    (
        Endpoint {
            available: Arc::clone(&available),
            ready: true,
            pending: false,
            protocol: 3,
        },
        Launcher {
            admitted,
            launches: AtomicUsize::new(0),
            available,
            ready_after,
            exited,
        },
        Timer(ManualClock::default()),
    )
}
fn compatibility() -> Compatibility<'static> {
    Compatibility {
        protocol: 3,
        version: "1.0.0",
    }
}
async fn connect(
    endpoint: &Endpoint,
    launcher: &Launcher,
    timer: &Timer,
    stages: &Mutex<Vec<ConnectStage>>,
) -> RuntimeStatus {
    connect_until_ready(
        endpoint,
        launcher,
        timer,
        ConnectOptions {
            compatibility: compatibility(),
            timeout: Duration::from_secs(5),
            exchange_timeout: Duration::from_secs(3),
        },
        |stage| stages.lock().unwrap().push(stage),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn reaped_first_launch_retries_inside_one_explicit_connection() {
    let (endpoint, launcher, timer) = setup(true, 2, true);
    let stages = Mutex::new(Vec::new());
    assert_eq!(
        connect(&endpoint, &launcher, &timer, &stages).await,
        RuntimeStatus::Ready
    );
    assert_eq!(launcher.launches.load(Ordering::Relaxed), 2);
    assert_eq!(
        *stages.lock().unwrap(),
        [ConnectStage::Starting, ConnectStage::PreparingEngine]
    );
}

#[tokio::test]
async fn busy_owner_and_unreaped_child_never_authorize_a_duplicate() {
    for (admitted, exited, expected) in [(false, true, 0), (true, false, 1)] {
        let (endpoint, launcher, timer) = setup(admitted, usize::MAX, exited);
        assert_eq!(
            connect(&endpoint, &launcher, &timer, &Mutex::new(Vec::new())).await,
            RuntimeStatus::Unavailable
        );
        assert_eq!(launcher.launches.load(Ordering::Relaxed), expected);
    }
}

#[tokio::test]
async fn repeatedly_exiting_children_have_exactly_three_launches() {
    let (endpoint, launcher, timer) = setup(true, usize::MAX, true);
    assert_eq!(
        connect(&endpoint, &launcher, &timer, &Mutex::new(Vec::new())).await,
        RuntimeStatus::Unavailable
    );
    assert_eq!(launcher.launches.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn answering_runtime_retains_authority_while_engine_is_unavailable() {
    let (mut endpoint, launcher, timer) = setup(true, usize::MAX, true);
    endpoint.available.store(true, Ordering::Relaxed);
    endpoint.ready = false;
    let stages = Mutex::new(Vec::new());
    assert_eq!(
        connect(&endpoint, &launcher, &timer, &stages).await,
        RuntimeStatus::Unavailable
    );
    assert_eq!(launcher.launches.load(Ordering::Relaxed), 0);
    assert_eq!(*stages.lock().unwrap(), [ConnectStage::PreparingEngine]);
}

#[tokio::test]
async fn incompatible_runtime_and_silent_pending_probe_never_launch() {
    let (mut endpoint, launcher, timer) = setup(true, usize::MAX, true);
    endpoint.available.store(true, Ordering::Relaxed);
    endpoint.protocol = 4;
    assert_eq!(
        connect(&endpoint, &launcher, &timer, &Mutex::new(Vec::new())).await,
        RuntimeStatus::Incompatible
    );
    endpoint.pending = true;
    assert_eq!(
        connect(&endpoint, &launcher, &timer, &Mutex::new(Vec::new())).await,
        RuntimeStatus::Unavailable
    );
    assert_eq!(launcher.launches.load(Ordering::Relaxed), 0);
}
