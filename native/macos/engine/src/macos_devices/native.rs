//! Native registration ownership and acknowledged teardown, never on DSP.
#[cfg(test)]
mod tests;
use super::{
    admission::{Admission, Permit},
    aggregate::Aggregate,
    buffer_control::{self, HostBufferControl},
    buffer_transaction::BufferPlan,
    host::Host,
    spec::Spec,
    worker::metrics::PcmMetrics,
};
use std::{
    ffi::c_void,
    ptr::NonNull,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};
// Match the graph's 256-node ceiling, counting BOTH current and quarantined
// native workers. Never release admission merely because a node was removed.
static NATIVE_OWNERS: Admission = Admission::new(256);
pub(super) struct Native {
    pub(super) log_id: u64,
    // The format monitor owns this reader cursor; native event publication never waits for it.
    pub(super) clock_trace_cursor: AtomicU64,
    pub(super) pointer: NonNull<c_void>,
    pub(super) group: Option<Arc<Aggregate>>,
    /// Performs this owner's control-side native operations.
    pub(super) host: Arc<dyn Host>,
    _permit: Permit<'static>,
    /// This process's client buffer, physical or virtual, follows the session block.
    requested_buffer: AtomicU32,
    pub(super) output_only: AtomicBool,
    // Stable Rust-owned callback slot. Drop runs only after native destroy has
    // acknowledged all callbacks, so the native device owner cannot outlive this box.
    pub(super) render: Box<super::direct::RenderSlot>,
    pub(super) pcm_metrics: PcmMetrics,
    pub(super) source_sample_faults: mixrelay_audio_io::pcm::SampleFaultCounter,
    pub(super) pcm: Box<super::worker::callback::PcmSlot>,
    pub(super) latency: super::latency::BridgeMeasurements,
}
impl Native {
    pub(super) fn active_owners() -> usize {
        NATIVE_OWNERS.in_use()
    }
    pub(super) fn create(
        spec: &Spec,
        rate: u32,
        group: Option<Arc<Aggregate>>,
        buffer_frames: u32,
        host: Arc<dyn Host>,
    ) -> Result<Self, String> {
        let permit = NATIVE_OWNERS
            .acquire()
            .ok_or("CoreAudio native owner limit reached; waiting for retired device callbacks")?;
        let uid = spec.native_uid()?;
        let pointer = host
            .create(spec, &uid, rate, group.as_deref())
            .ok_or("invalid CoreAudio channels or native allocation failed")?;
        let native = Self {
            log_id: super::bridge::NEXT_DEVICE_EPOCH.fetch_add(1, Ordering::Relaxed),
            clock_trace_cursor: AtomicU64::new(0),
            pointer,
            group,
            host,
            _permit: permit,
            requested_buffer: AtomicU32::new(buffer_frames),
            output_only: AtomicBool::new(false),
            render: Box::default(),
            pcm_metrics: PcmMetrics::default(),
            source_sample_faults: mixrelay_audio_io::pcm::SampleFaultCounter::default(),
            pcm: Box::default(),
            latency: super::latency::BridgeMeasurements::default(),
        };
        tracing::info!(owner = native.log_id, native_uid = %uid.to_string_lossy(),
            input = spec.input, channels = spec.channels, "CoreAudio native owner prepared");
        unsafe {
            super::ffi::mr_device_io_pcm_hook(
                native.pointer.as_ptr(),
                std::ptr::from_ref(native.pcm.as_ref()).cast_mut().cast(),
                super::worker::callback::dispatch,
            );
            super::ffi::mr_device_io_render_hook(
                native.pointer.as_ptr(),
                std::ptr::from_ref(native.render.as_ref()).cast_mut().cast(),
                super::direct::dispatch,
            );
        }
        Ok(native)
    }
    pub(super) fn request_buffer(&self, frames: u32) {
        self.requested_buffer.store(frames, Ordering::Release);
    }
    #[cfg(any(test, feature = "native-lab"))]
    pub(super) fn requested_buffer(&self) -> u32 {
        self.requested_buffer.load(Ordering::Acquire)
    }
    pub(super) fn connect(&self, stop: &AtomicBool) -> Result<(), String> {
        // SAFETY: this control owner retains DeviceIo and the previous registration
        // has drained. Direct-capable graphs retain native input on output IOProcs.
        unsafe {
            &*self
                .pointer
                .as_ptr()
                .cast::<mixrelay_macos_transport::device_io::DeviceIo>()
        }
        .set_output_only(self.output_only.load(Ordering::Acquire));
        let requested = self.requested_buffer.load(Ordering::Acquire);
        let setting = if requested == 0 {
            None
        } else {
            tracing::debug!(
                owner = self.log_id,
                requested,
                "preparing CoreAudio client buffer"
            );
            let (device, previous) = buffer_control::prepare_initial(self, requested, stop)?;
            let plan = BufferPlan::prepare([(device, previous)], requested)
                .map_err(|error| format!("CoreAudio buffer preparation failed: {error:?}"))?;
            plan.apply(&mut HostBufferControl(&*self.host))
                .map_err(|error| format!("CoreAudio buffer application failed: {error:?}"))?;
            Some((device, plan))
        };
        let expected = setting.as_ref().map_or(0, |(device, _)| *device);
        tracing::debug!(
            owner = self.log_id,
            expected,
            "registering CoreAudio callback"
        );
        let status = self.host.connect(self, expected);
        tracing::debug!(
            owner = self.log_id,
            status,
            "CoreAudio callback registration returned"
        );
        if status != 0 {
            let restored = setting
                .as_ref()
                .is_none_or(|(_, plan)| plan.restore(&mut HostBufferControl(&*self.host)));
            return Err(format!(
                "CoreAudio connection failed: {} ({status}); buffer restored={restored}",
                connection_failure(status)
            ));
        }
        Ok(())
    }
}

fn connection_failure(status: i32) -> &'static str {
    match status {
        -1 => "previous callback registration not drained",
        -2 => "selected device or private group unavailable",
        -3 => "native stream format or channel layout unavailable",
        -5 => "selected device changed during preparation",
        _ => "native callback registration or start rejected",
    }
}
pub(super) struct Connection<'a> {
    pub(super) native: &'a Native,
    pub(super) failed: &'a AtomicBool,
    pub(super) clock_identity: &'a AtomicU32,
}
impl Drop for Connection<'_> {
    fn drop(&mut self) {
        self.failed.store(true, Ordering::Release);
        self.clock_identity.store(0, Ordering::Release);
        // Keep the original owner/context alive until CoreAudio acknowledges
        // unregistering. Reopening over a failed teardown would race callbacks.
        while !self.native.host.disconnect(self.native) {
            thread::park_timeout(Duration::from_millis(100));
        }
    }
}
// SAFETY: the owning native worker exclusively connects/disconnects/queries.
// Native callback and graph own opposite SPSC cursors. Prepared SRC is pinned in
// CallbackBridge, not this Arc; PcmSlot announces readers and excludes concurrent
// mutable access. Control drains publication before replacement or releasing SRC.
// The last Arc is destroyed only after control and native callbacks stop.
unsafe impl Send for Native {}
unsafe impl Sync for Native {}
impl Drop for Native {
    fn drop(&mut self) {
        // Normally the connection guard already drained registration. Even if
        // that invariant is violated, retain the group AND admission until the
        // native destructor acknowledges release; a debug assertion is not a
        // lifetime guarantee in release builds. This destructor is never on DSP.
        while !self.host.destroy(self) {
            thread::park_timeout(Duration::from_millis(100));
        }
    }
}
