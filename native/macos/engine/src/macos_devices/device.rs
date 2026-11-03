//! Graph-facing queue handle and native worker lifetime.
mod continuity;
pub(super) use continuity::InputContinuity;

use super::{
    activity::BridgeActivity,
    aggregate::Aggregate,
    bridge::{DeviceBridge, NEXT_DEVICE_EPOCH},
    ffi::{
        mr_device_io_graph_read, mr_device_io_graph_write, mr_device_io_invalid,
        mr_device_io_output_epoch,
    },
    host::Host,
    native::Native,
    spec::Spec,
    worker::WorkerContext,
};
use mixrelay_audio_io::HostAudioBackendError;
use mixrelay_audio_io::process::{InputActivity, TransferOutcome};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread,
};
pub(super) struct Device {
    pub(super) input_continuity: InputContinuity,
    pub(super) activity: Arc<BridgeActivity>,
    pub(super) identity: u64,
    pub(super) native: Arc<Native>,
    pub(super) channels: usize,
    pub(super) input: bool,
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
    pub(super) worker: Option<thread::JoinHandle<()>>,
}

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct CallbackConfig {
    pub(super) buffer_frames: u32,
    /// False keeps input available for a graph's direct duplex renderer.
    pub(super) output_only: bool,
}

impl Device {
    #[cfg(any(test, feature = "native-lab"))]
    pub(super) fn open(
        spec: &Spec,
        rate: u32,
        host: Arc<dyn Host>,
    ) -> Result<Self, HostAudioBackendError> {
        Self::open_with_group(
            spec,
            rate,
            None,
            CallbackConfig {
                buffer_frames: 0,
                output_only: !spec.input,
            },
            host,
        )
    }
    pub(super) fn open_with_group(
        spec: &Spec,
        rate: u32,
        group: Option<Arc<Aggregate>>,
        callback: CallbackConfig,
        host: Arc<dyn Host>,
    ) -> Result<Self, HostAudioBackendError> {
        if spec.input {
            super::permission::authorize(&*host)?;
        }
        let permission = spec.input.then(|| host.input_permission());
        if let Some(permission) = permission {
            tracing::info!(?permission, "CoreAudio input consent before initialization");
        }
        let native = Arc::new(
            Native::create(spec, rate, group, callback.buffer_frames, host)
                .map_err(super::backend_error)?,
        );
        native
            .output_only
            .store(callback.output_only, Ordering::Release);
        let buffer_frames = callback.buffer_frames;
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(true));
        let epoch = Arc::new(AtomicU64::new(0));
        let graph_rate = Arc::new(AtomicU32::new(rate));
        let graph_quantum = Arc::new(AtomicU32::new(buffer_frames));
        let ready_rate = Arc::new(AtomicU32::new(0));
        let native_rate = Arc::new(AtomicU32::new(0));
        let clock_identity = Arc::new(AtomicU32::new(0));
        let bridge_ready = Arc::new(AtomicBool::new(false));
        let pending_bridge = Arc::new(Mutex::new(None));
        let clock_aligned = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let activity = Arc::clone(&native.render.activity);
        let context = WorkerContext {
            activity: Arc::clone(&activity),
            native: native.clone(),
            stop: stop.clone(),
            failed: failed.clone(),
            epoch: epoch.clone(),
            graph_rate: graph_rate.clone(),
            graph_quantum: graph_quantum.clone(),
            ready_rate: ready_rate.clone(),
            native_rate: native_rate.clone(),
            clock_identity: clock_identity.clone(),
            bridge_ready: bridge_ready.clone(),
            pending_bridge: pending_bridge.clone(),
            clock_aligned: clock_aligned.clone(),
            channels: usize::from(spec.channels),
            input: spec.input,
        };
        let worker = thread::Builder::new()
            .name("mixrelay-coreaudio-device".into())
            .spawn(move || context.run(&tx))
            .map_err(|error| super::backend_error(error.to_string()))?;
        let result = Self {
            input_continuity: InputContinuity::default(),
            activity,
            identity: NEXT_DEVICE_EPOCH.fetch_add(1, Ordering::Relaxed),
            native,
            channels: usize::from(spec.channels),
            input: spec.input,
            stop,
            failed,
            epoch,
            graph_rate,
            graph_quantum,
            ready_rate,
            native_rate,
            clock_identity,
            bridge_ready,
            pending_bridge,
            clock_aligned,
            worker: Some(worker),
        };
        match super::permission::wait_for_native(&*result.native.host, permission, |timeout| {
            rx.recv_timeout(timeout)
        }) {
            Ok(()) => Ok(result),
            Err(error) => {
                result.stop.store(true, Ordering::Release);
                // Drop transfers the worker to tracked retirement; a timeout
                // cannot cancel a native call or release its callback allocation.
                Err(error)
            }
        }
    }
    /// This device's latest callback label (its mean frame) in host ticks, or zero before any.
    pub(super) fn callback_host(&self) -> u64 {
        // SAFETY: this owner retains Native; only an atomic is read.
        unsafe { super::ffi::mr_device_io_callback_host(self.native.pointer.as_ptr()) }
    }
    /// `host` labels this graph cycle's transfers; see `DeviceSet::graph_host`.
    pub(super) fn read(&self, samples: &mut [f32], host: u64) -> TransferOutcome {
        self.read_timed(samples, host, None)
    }

    pub(super) fn read_timed(
        &self,
        samples: &mut [f32],
        host: u64,
        time: Option<(u64, u32)>,
    ) -> TransferOutcome {
        samples.fill(0.0);
        let source_sample_faults = if self.input {
            self.native.source_sample_faults.take()
        } else {
            0
        };
        if !self.input || samples.is_empty() || !samples.len().is_multiple_of(self.channels) {
            return TransferOutcome {
                source_sample_faults,
                ..TransferOutcome::complete(0)
            };
        }
        if !self.ready() {
            return self.input_continuity.lost(source_sample_faults);
        }
        let frames = u32::try_from(samples.len() / self.channels).unwrap_or(0);
        if !self.activity.ready() {
            return TransferOutcome {
                source_sample_faults,
                ..super::recovery::offline_transfer(samples.len(), self.channels)
            };
        }
        // SAFETY: this serialized graph consumer retains Native and owns the
        // complete writable PCM span, disjoint from the producer's queue storage.
        let read = if let Some((first, rate)) = time {
            unsafe {
                mixrelay_macos_transport::device_io::graph_read_at(
                    self.native.pointer.as_ptr().cast(),
                    samples,
                    self.activity.read_epoch(self.epoch.load(Ordering::Acquire)),
                    (host, first, rate),
                )
            }
        } else {
            unsafe {
                mr_device_io_graph_read(
                    self.native.pointer.as_ptr(),
                    samples.as_mut_ptr(),
                    frames,
                    self.activity.read_epoch(self.epoch.load(Ordering::Acquire)),
                    host,
                )
            }
        };
        TransferOutcome {
            source_sample_faults,
            frames: read.frames,
            discontinuity: self.input_continuity.admit(read),
            activity: if read.initial_epoch && read.frames < frames {
                // The first freshness reset admits a timeline; it is not loss
                // from an already running input. The next shortage is Active.
                InputActivity::Priming
            } else {
                InputActivity::Active
            },
        }
    }
    pub(super) fn write(&self, samples: &[f32], host: u64) -> TransferOutcome {
        if self.input || !self.ready() || !samples.len().is_multiple_of(self.channels) {
            return TransferOutcome::complete(0);
        }
        let frames = u32::try_from(samples.len() / self.channels).unwrap_or(0);
        if !self.activity.ready() {
            return super::recovery::offline_transfer(samples.len(), self.channels);
        }
        TransferOutcome::complete(unsafe {
            mr_device_io_graph_write(self.native.pointer.as_ptr(), samples.as_ptr(), frames, host)
        })
    }
    pub(super) fn ready(&self) -> bool {
        // SAFETY: this graph handle retains Native; the observation reads only
        // its stable invalid atomic, never mutable native connection/callback state.
        let invalid = unsafe { mr_device_io_invalid(self.native.pointer.as_ptr()) };
        !invalid
            && !self.failed.load(Ordering::Acquire)
            && self.ready_rate.load(Ordering::Acquire) == self.graph_rate.load(Ordering::Acquire)
    }

    pub(super) fn set_active(&self, enabled: bool) {
        if self.activity.request(enabled) && !self.input && !enabled {
            // SAFETY: the retained native owner accepts an atomic epoch request.
            // Its IOProc remains the only consumer that discards output PCM.
            unsafe {
                mr_device_io_output_epoch(
                    self.native.pointer.as_ptr(),
                    NEXT_DEVICE_EPOCH.fetch_add(1, Ordering::Relaxed),
                );
            }
        }
    }
}
impl Drop for Device {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            super::worker_retirement::WORKERS.retire(worker);
        }
    }
}
