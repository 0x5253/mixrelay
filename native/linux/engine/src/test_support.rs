//! Shared test doubles and owned native fixtures.
use crate::{
    backend::LinuxVirtualEndpointBackend, format::EndpointObservation, runtime::EndpointDriver,
    runtime::render_format,
};
use mixrelay_driver_core::{DriverCommand, DriverError, DriverSnapshot, DriverState};
use std::sync::{Arc, Mutex};

/// A backend with no reachable Driver, or the lab's real Driver under `native-lab`.
pub(crate) fn test_backend() -> Arc<LinuxVirtualEndpointBackend> {
    LinuxVirtualEndpointBackend::new(
        test_driver(),
        render_format(),
        Arc::new(EndpointObservation::default()),
    )
}

pub(crate) fn test_driver() -> Arc<dyn EndpointDriver> {
    #[cfg(feature = "native-lab")]
    struct Driver(mixrelay_driver_adapters::DriverClient);
    #[cfg(not(feature = "native-lab"))]
    struct Driver;
    impl EndpointDriver for Driver {
        fn snapshot(&self) -> Result<DriverSnapshot, DriverError> {
            #[cfg(feature = "native-lab")]
            return self.0.snapshot_blocking();
            #[cfg(not(feature = "native-lab"))]
            Err(DriverError::Unavailable("fixture has no Driver".into()))
        }
        fn mutate(&self, command: &DriverCommand) -> Result<DriverSnapshot, DriverError> {
            #[cfg(feature = "native-lab")]
            return self.0.mutate_blocking(command);
            #[cfg(not(feature = "native-lab"))]
            {
                let _ = command;
                Err(DriverError::Unavailable("fixture has no Driver".into()))
            }
        }
    }
    #[cfg(feature = "native-lab")]
    return Arc::new(Driver(
        mixrelay_driver_adapters::DriverClient::new().unwrap(),
    ));
    #[cfg(not(feature = "native-lab"))]
    Arc::new(Driver)
}

/// Serves a fixed snapshot and records every mutation without applying it.
pub(crate) struct FakeDriver {
    snapshot: Mutex<DriverSnapshot>,
    mutations: Mutex<Vec<DriverCommand>>,
}

impl FakeDriver {
    pub(crate) fn new(snapshot: DriverSnapshot) -> Arc<Self> {
        Arc::new(Self {
            snapshot: Mutex::new(snapshot),
            mutations: Mutex::default(),
        })
    }

    pub(crate) fn set_state(&self, state: DriverState) {
        self.snapshot.lock().unwrap().state = state;
    }

    pub(crate) fn mutations(&self) -> Vec<DriverCommand> {
        self.mutations.lock().unwrap().clone()
    }
}

impl EndpointDriver for FakeDriver {
    fn snapshot(&self) -> Result<DriverSnapshot, DriverError> {
        Ok(self.snapshot.lock().unwrap().clone())
    }

    fn mutate(&self, command: &DriverCommand) -> Result<DriverSnapshot, DriverError> {
        self.mutations.lock().unwrap().push(command.clone());
        self.snapshot()
    }
}

/// A child process this test owns; it is terminated and reaped on drop.
pub(crate) struct FixtureProcess(pub(crate) std::process::Child);

impl FixtureProcess {
    pub(crate) fn id(&self) -> u32 {
        self.0.id()
    }

    pub(crate) fn start(path: &std::path::Path, arguments: &[&str]) -> Self {
        Self(
            std::process::Command::new(path)
                .args(arguments)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::inherit())
                .spawn()
                .expect("owned application fixture"),
        )
    }

    #[cfg(feature = "native-lab")]
    pub(crate) fn report(mut self) -> serde_json::Value {
        self.stop();
        serde_json::from_reader(self.0.stdout.take().unwrap()).unwrap()
    }

    #[cfg(feature = "native-lab")]
    pub(crate) fn signal(&mut self, signal: i32) {
        assert!(self.0.try_wait().unwrap().is_none());
        let pid = i32::try_from(self.0.id()).unwrap();
        // SAFETY: this live unreaped child remains ours throughout the call.
        assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
    }

    pub(crate) fn stop(&mut self) {
        if self.0.try_wait().ok().flatten().is_some() {
            return;
        }
        if let Ok(pid) = i32::try_from(self.0.id()) {
            // SAFETY: child is still owned/unreaped, so its PID cannot be reused.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
        for _ in 0..50 {
            if self.0.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for FixtureProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The production graph owns an always-processing filter. Standalone tap tests
/// need the same owner: a passive monitor is not itself the graph's clock driver.
#[cfg(feature = "native-lab")]
pub(crate) struct ObservedDriver {
    _driver: mixrelay_linux_pipewire::GraphDriver<ClockObserver>,
    pub(crate) clock: Arc<std::sync::atomic::AtomicU64>,
}

#[cfg(feature = "native-lab")]
struct ClockObserver(Arc<std::sync::atomic::AtomicU64>);

#[cfg(feature = "native-lab")]
impl mixrelay_linux_pipewire::DriverProcess for ClockObserver {
    fn process(&mut self, cycle: &mixrelay_linux_pipewire::DriverCycle) {
        self.0.store(
            u64::from(cycle.clock_id) << 32 | u64::from(cycle.rate),
            std::sync::atomic::Ordering::Release,
        );
    }
}

#[cfg(feature = "native-lab")]
impl ObservedDriver {
    pub(crate) fn new() -> Self {
        let clock = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let driver = mixrelay_linux_pipewire::GraphDriver::open(
            ClockObserver(Arc::clone(&clock)),
            mixrelay_linux_pipewire::DriverFormat::new(48_000, 128).unwrap(),
        )
        .unwrap();
        Self {
            _driver: driver,
            clock,
        }
    }
}
