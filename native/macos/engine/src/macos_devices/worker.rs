//! Native control supervisor; prepared PCM executes only on the device callback.
use super::{
    bridge::DeviceBridge,
    ffi::mr_device_io_native_rate,
    native::{Connection, Native},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
pub(super) mod callback;
pub(super) mod metrics;
pub(super) struct WorkerContext {
    pub(super) activity: Arc<super::activity::BridgeActivity>,
    pub(super) native: Arc<Native>,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) failed: Arc<AtomicBool>,
    pub(super) epoch: Arc<AtomicU64>,
    pub(super) graph_rate: Arc<AtomicU32>,
    pub(super) graph_quantum: Arc<AtomicU32>,
    pub(super) ready_rate: Arc<AtomicU32>,
    pub(super) native_rate: Arc<AtomicU32>,
    pub(super) clock_identity: Arc<AtomicU32>,
    pub(super) bridge_ready: Arc<AtomicBool>,
    pub(super) pending_bridge: Arc<Mutex<Option<DeviceBridge>>>,
    pub(super) clock_aligned: Arc<AtomicBool>,
    pub(super) channels: usize,
    pub(super) input: bool,
}

impl WorkerContext {
    pub(super) fn run(self, tx: &std::sync::mpsc::SyncSender<Result<(), String>>) {
        let span = tracing::info_span!(
            "coreaudio_device",
            owner = self.native.log_id,
            input = self.input,
            channels = self.channels
        );
        let _entered = span.enter();
        mixrelay_engine_host::device_worker::supervise(&self.stop, &self.failed, tx, |connected| {
            tracing::info!(
                previous_native_rate = self.native_rate.load(Ordering::Acquire),
                processing_rate = self.graph_rate.load(Ordering::Acquire),
                "CoreAudio connection starting"
            );
            let result = self.connection(connected);
            match &result {
                Err(error) if !self.stop.load(Ordering::Acquire) => {
                    tracing::warn!(%error, "CoreAudio connection ended; recovery pending");
                }
                _ => tracing::info!("CoreAudio connection retired"),
            }
            result
        });
    }
    pub(super) fn connection(&self, connected: &mut dyn FnMut()) -> Result<(), String> {
        let pointer = self.native.pointer.as_ptr();
        let _connection = Connection {
            native: &self.native,
            failed: &self.failed,
            clock_identity: &self.clock_identity,
        };
        self.ready_rate.store(0, Ordering::Release);
        self.native.connect(&self.stop)?;
        let native_rate = unsafe { mr_device_io_native_rate(pointer) };
        self.native_rate.store(native_rate, Ordering::Release);
        // Only the connection owner queries CoreAudio. The graph uses this
        // atomic proof, not a UID string, nominal Hz or a property call on RT.
        self.clock_identity.store(
            self.native.host.clock_device(&self.native),
            Ordering::Release,
        );
        self.serve_callback(native_rate, connected)
    }

    fn serve_callback(&self, native_rate: u32, connected: &mut dyn FnMut()) -> Result<(), String> {
        let mut owner = callback::CallbackBridge::new(self, native_rate)?;
        let mut format = (
            self.graph_rate.load(Ordering::Acquire),
            self.clock_aligned.load(Ordering::Acquire),
        );
        let mut ticks = self.native.host.ticks(&self.native);
        let mut last_progress = Instant::now();
        let mut activated = false;
        while !self.stop.load(Ordering::Acquire) {
            let requested = (
                self.graph_rate.load(Ordering::Acquire),
                self.clock_aligned.load(Ordering::Acquire),
            );
            let pending = self
                .pending_bridge
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if pending.is_some() || requested != format {
                let prepared = match pending {
                    Some(bridge)
                        if bridge.native_rate == native_rate
                            && (bridge.graph_rate, bridge.clock_aligned) == requested =>
                    {
                        bridge
                    }
                    _ => DeviceBridge::prepare(
                        self.channels,
                        self.input,
                        native_rate,
                        requested.0,
                        requested.1,
                    )?,
                };
                owner.replace(prepared);
                format = requested;
            }
            if !activated && self.ready_rate.load(Ordering::Acquire) != 0 {
                connected();
                activated = true;
            }
            if let Some(error) = self.native.pcm.failure() {
                return Err(error.into());
            }
            if self.native.render.reconnect.swap(false, Ordering::AcqRel) {
                return Err("CoreAudio callback requested topology recovery".into());
            }
            // Native property IPC stays on control; it cannot block callback PCM.
            if !self.native.host.alive(&self.native) {
                return Err("CoreAudio device lost or reconfigured".into());
            }
            let next = self.native.host.ticks(&self.native);
            if next != ticks {
                ticks = next;
                last_progress = Instant::now();
            } else if last_progress.elapsed() >= Duration::from_secs(2) {
                return Err("CoreAudio device stopped delivering audio callbacks".into());
            }
            thread::park_timeout(Duration::from_millis(if activated { 50 } else { 1 }));
        }
        // owner drains before the enclosing Connection can unregister/free storage.
        Ok(())
    }
}
