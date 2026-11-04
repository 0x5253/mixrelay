//! Unpublished native owners for tests that exercise coordination without a HAL device.
use super::{
    bridge::NEXT_DEVICE_EPOCH,
    device::{Device, InputContinuity},
    native::Native,
    simulated::SimulatedHost,
    spec::Spec,
    worker::WorkerContext,
};
use mixrelay_audio_graph::DeviceSelector;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub(super) fn synthetic_output(
    channels: u16,
    graph_rate: u32,
    native_rate: u32,
) -> (Arc<Device>, WorkerContext) {
    synthetic_device(channels, graph_rate, native_rate, false)
}

pub(super) fn synthetic_device(
    channels: u16,
    graph_rate: u32,
    native_rate: u32,
    input: bool,
) -> (Arc<Device>, WorkerContext) {
    let native = Arc::new(
        Native::create(
            &Spec {
                selector: DeviceSelector::PersistentUid {
                    uid: "synthetic-no-device".into(),
                },
                input,
                channels,
            },
            native_rate,
            None,
            0,
            SimulatedHost::new(),
        )
        .unwrap(),
    );
    // SAFETY: this fresh synthetic owner never connects and no callback/queue
    // role is published yet. Fixtures supply the matching interleaved Float32 ABL themselves.
    assert!(unsafe {
        mixrelay_macos_transport::device_io::test_observation::admit_float_callback(
            native.pointer.as_ptr().cast(),
        )
    });
    let context = WorkerContext {
        activity: Arc::clone(&native.render.activity),
        native: native.clone(),
        stop: Arc::new(AtomicBool::new(false)),
        failed: Arc::new(AtomicBool::new(false)),
        epoch: Arc::new(AtomicU64::new(1)),
        graph_rate: Arc::new(AtomicU32::new(graph_rate)),
        graph_quantum: Arc::new(AtomicU32::new(32)),
        ready_rate: Arc::new(AtomicU32::new(graph_rate)),
        native_rate: Arc::new(AtomicU32::new(native_rate)),
        clock_identity: Arc::new(AtomicU32::new(0)),
        bridge_ready: Arc::new(AtomicBool::new(false)),
        pending_bridge: Arc::new(Mutex::new(None)),
        clock_aligned: Arc::new(AtomicBool::new(false)),
        channels: usize::from(channels),
        input,
    };
    let device = Arc::new(Device {
        input_continuity: InputContinuity::default(),
        activity: Arc::clone(&context.activity),
        identity: NEXT_DEVICE_EPOCH.fetch_add(1, Ordering::Relaxed),
        native,
        channels: context.channels,
        input,
        stop: context.stop.clone(),
        failed: context.failed.clone(),
        epoch: context.epoch.clone(),
        graph_rate: context.graph_rate.clone(),
        graph_quantum: context.graph_quantum.clone(),
        ready_rate: context.ready_rate.clone(),
        native_rate: context.native_rate.clone(),
        clock_identity: context.clock_identity.clone(),
        bridge_ready: context.bridge_ready.clone(),
        pending_bridge: context.pending_bridge.clone(),
        clock_aligned: context.clock_aligned.clone(),
        worker: None,
    });
    (device, context)
}

pub(super) fn wait_until(mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "native device transition stalled"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
