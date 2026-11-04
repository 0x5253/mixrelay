//! Own a real Driver endpoint and its free control API inside the private native lab.
use mixrelay_audio_io::VirtualEndpointSpec;
use mixrelay_driver_adapters::FileSettingsStore;
use mixrelay_driver_core::{DriverDeviceConfig, DriverManager};
use mixrelay_local_socket::{AuthorizedListener, DRIVER_SOCKET, Peer, bind, current_uid};
use std::{
    fs::OpenOptions,
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    path::Path,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

const DEADLINE: Duration = Duration::from_secs(8);

pub(super) struct DriverFixture {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    finished: mpsc::Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl DriverFixture {
    pub(super) fn start(directory: &Path, spec: VirtualEndpointSpec) -> Self {
        let socket_directory = mixrelay_local_socket::socket_directory().unwrap();
        assert_eq!(socket_directory, directory.join("ipc"));
        let path = directory.join("forward-driver.json");
        let device = DriverDeviceConfig {
            spec,
            render_format: None,
            capture_format: None,
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        serde_json::to_writer(
            &mut file,
            &serde_json::json!({"revision": 0, "active": true, "devices": [device]}),
        )
        .unwrap();
        file.flush().unwrap();
        drop(file);
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let (ready, startup) = mpsc::sync_channel(1);
        let (finished, completion) = mpsc::sync_channel(1);
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let uid = current_uid();
                let (listener, owner) = bind(&socket_directory, DRIVER_SOCKET, uid).await.unwrap();
                let listener = AuthorizedListener::new(listener, owner, vec![uid]);
                let manager = DriverManager::open(
                    mixrelay_linux_driver::LinuxDriver::open().unwrap(),
                    FileSettingsStore::new(path),
                    mixrelay_driver_core::DriverRuntime::new(
                        mixrelay_driver_core::DriverProcessId::new(std::process::id()).unwrap(),
                        Box::new(mixrelay_time::StdClock::new()),
                        Box::new(mixrelay_driver_adapters::SystemLatencyGenerations),
                    ),
                )
                .unwrap();
                let router = mixrelay_driver_adapters::router(
                    manager,
                    mixrelay_driver_adapters::InventoryIntervals::default(),
                );
                ready.send(()).unwrap();
                axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<Peer>(),
                )
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
            });
            drop(runtime);
            let _ = finished.send(());
        });
        let fixture = Self {
            stop: Some(stop),
            finished: completion,
            thread: Some(thread),
        };
        startup
            .recv_timeout(DEADLINE)
            .expect("private Driver startup");
        fixture
    }
}

impl Drop for DriverFixture {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if self.finished.recv_timeout(DEADLINE).is_err() {
            assert!(thread::panicking(), "private Driver did not drain");
            return;
        }
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            assert!(
                result.is_ok() || thread::panicking(),
                "private Driver failed"
            );
        }
    }
}
