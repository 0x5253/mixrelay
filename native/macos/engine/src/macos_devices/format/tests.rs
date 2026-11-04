use super::{PreparedDeviceFormat, PublishedFormat};
use crate::macos_devices::{
    DeviceSet, MacDevices, bridge::DeviceBridge, device::Device, device_fixture::synthetic_output,
    ffi::mr_device_io_queued, handoff::ResourceHandoff, simulated::SimulatedHost, spec::Spec,
};
use mixrelay_audio_graph::{DeviceSelector, GraphDocument, NodeConfig, RenderFormat};
use mixrelay_engine_core::{
    GraphResourceBackend, PreparedGraphFormat, session::AdapterRuntimeStatus,
};
use std::{
    sync::{Arc, atomic::Ordering},
    thread,
};

/// A coordinator over a simulated `CoreAudio` publishing one stereo device, with `device`
/// cached as the owner of the returned graph's output. Graph and cache select the
/// published device by the same persistent UID, so preparation reuses `device`.
fn format_fixture(device: &Arc<Device>) -> (MacDevices, GraphDocument) {
    // This unpublished owner represents a graph with only an output, so
    // its cached registration intent must match normal graph admission.
    device.native.output_only.store(true, Ordering::Release);
    let host = SimulatedHost::new();
    let selector = DeviceSelector::PersistentUid {
        uid: host.plug("io.mixrelay.test.format", 2, 48_000, 0.0),
    };
    let status = Arc::new(AdapterRuntimeStatus::new(
        crate::macos::adapter_profile(),
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    ));
    let resources = MacDevices::with_host(status, 48_000, 256, host);
    resources.cache.lock().unwrap().insert(
        "output".into(),
        (
            Spec {
                selector: selector.clone(),
                input: false,
                channels: 2,
            },
            Arc::downgrade(device),
        ),
    );
    let graph = GraphDocument::new(
        1,
        vec![mixrelay_audio_graph::NodeSpec {
            control: mixrelay_audio_graph::NodeControl::default(),
            id: "output".into(),
            label: None,
            config: NodeConfig::AudioDeviceOutput(mixrelay_audio_graph::AudioDeviceOutputConfig {
                selector,
                input_channels: 2,
            }),
        }],
        vec![],
    );
    (resources, graph)
}

#[test]
fn format_reservation_cancellation_and_reclaim_are_safe() {
    let (device, _context) = synthetic_output(2, 48_000, 44_100);
    let (resources, graph) = format_fixture(&device);
    let old = RenderFormat {
        sample_rate: 48_000,
        block_frames: 256,
    };
    let new = RenderFormat {
        sample_rate: 96_000,
        block_frames: 256,
    };
    assert_eq!(device.write(&[0.25; 512], 0).frames, 256);
    let cancelled = resources
        .prepare_format_change(
            &graph,
            mixrelay_engine_core::PlaybackBindings::Preserve,
            old,
            new,
        )
        .unwrap();
    assert!(!resources.handoff.idle());
    assert!(
        resources
            .prepare_format_change(
                &graph,
                mixrelay_engine_core::PlaybackBindings::Preserve,
                old,
                new
            )
            .is_err()
    );
    resources.reclaim();
    assert!(
        !resources.handoff.idle(),
        "recovery must not take a format reservation"
    );
    assert_eq!(device.graph_rate.load(Ordering::Acquire), 48_000);
    assert_eq!(
        unsafe { mr_device_io_queued(device.native.pointer.as_ptr(), true) },
        256
    );
    drop(cancelled);
    assert!(
        resources.handoff.idle(),
        "cancellation releases resource preparation"
    );
    assert!(device.pending_bridge.lock().unwrap().is_none());
    resources
        .prepare_format_change(
            &graph,
            mixrelay_engine_core::PlaybackBindings::Preserve,
            old,
            new,
        )
        .unwrap()
        .commit();
    assert_eq!(device.graph_rate.load(Ordering::Acquire), 96_000);
}

#[test]
fn committed_format_rejects_mismatched_pcm_and_activates_pending_slot() {
    let (device, context) = synthetic_output(2, 48_000, 44_100);
    let (resources, graph) = format_fixture(&device);
    let old = RenderFormat {
        sample_rate: 48_000,
        block_frames: 256,
    };
    let new = RenderFormat {
        sample_rate: 96_000,
        block_frames: 256,
    };
    assert_eq!(device.write(&[0.25; 512], 0).frames, 256);
    resources
        .prepare_format_change(
            &graph,
            mixrelay_engine_core::PlaybackBindings::Preserve,
            old,
            new,
        )
        .unwrap()
        .commit();
    assert!(
        !resources.handoff.idle(),
        "commit retains the graph reservation until activation"
    );
    assert_eq!(
        (
            device.graph_rate.load(Ordering::Acquire),
            device.native_rate.load(Ordering::Acquire),
        ),
        (96_000, 44_100),
        "publishing the processing rate must preserve the physical device rate"
    );
    assert_eq!(
        device.write(&[0.25; 512], 0).frames,
        0,
        "old converter must not accept new-rate PCM"
    );
    assert!(
        resources
            .prepare_format_change(
                &graph,
                mixrelay_engine_core::PlaybackBindings::Preserve,
                new,
                old
            )
            .is_err(),
        "pending publication slot cannot be replaced"
    );
    let mut prepared = device.pending_bridge.lock().unwrap().take().unwrap();
    prepared.activate(&context);
    assert_eq!(
        unsafe { mr_device_io_queued(device.native.pointer.as_ptr(), true) },
        0
    );
    assert_eq!(
        (prepared.graph_rate, prepared.native_rate),
        (96_000, 44_100)
    );
}

#[test]
fn prepared_bridge_and_graph_rate_publish_as_one_worker_observation() {
    let (device, _) = synthetic_output(2, 48_000, 44_100);
    let other = device.clone();
    let worker = thread::spawn(move || {
        loop {
            let mut pending = other.pending_bridge.lock().unwrap();
            if let Some(bridge) = pending.take() {
                assert_eq!(other.graph_rate.load(Ordering::Acquire), bridge.graph_rate);
                return;
            }
            drop(pending);
            thread::yield_now();
        }
    });
    Box::new(PreparedDeviceFormat {
        reservation: ResourceHandoff::<DeviceSet>::default()
            .reserve_format()
            .unwrap(),
        devices: vec![(
            device,
            DeviceBridge::prepare(2, false, 44_100, 96_000, false).unwrap(),
        )],
        published: Arc::new(PublishedFormat::new(48_000, 256)),
        requested_rate: 96_000,
        requested_buffer: 256,
        previous_buffer: 256,
    })
    .commit();
    worker.join().unwrap();
}
