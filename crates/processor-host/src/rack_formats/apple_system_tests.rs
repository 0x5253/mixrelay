//! Probe-to-worker checks against Audio Units that ship with macOS. They need neither
//! hardware nor a vendor authorization.

use truce_rack_core::scanner::PluginScanner;

use super::{
    adapters,
    identity::{RackFormat, RackLocator},
    worker::RackWorkerPlugin,
};

fn system_audio_unit(name: &str) -> RackLocator {
    let info = truce_rack_au::AuScanner::new()
        .scan()
        .unwrap()
        .into_iter()
        .find(|info| info.name.contains(name))
        .expect("macOS ships the system Audio Unit fixture");
    RackLocator::from_info(RackFormat::Au, info)
}

fn default_state() -> crate::state::InitialState {
    crate::state::InitialState::resolve(serde_json::json!({}), None).unwrap()
}

#[test]
fn apple_limiter_native_state_restores_a_changed_parameter() {
    // This checks native ClassInfo bytes, not a mock JSON parameter round trip.
    let locator = system_audio_unit("AUPeakLimiter");
    let record = adapters::probe(locator.clone()).unwrap();
    let parameter = record
        .descriptor
        .parameters
        .iter()
        .find(|parameter| !parameter.is_read_only)
        .expect("limiter exposes a writable parameter");
    let layout = record
        .descriptor
        .layouts
        .iter()
        .find(|layout| layout.layout_id == record.descriptor.default_layout_id)
        .unwrap();
    let mut first =
        RackWorkerPlugin::load(&locator, layout, 48_000.0, 128, &default_state()).unwrap();
    first.set_parameter(parameter.id, 0.37).unwrap();
    let range = first
        .parameter_ranges
        .iter()
        .find(|range| range.id == parameter.id)
        .copied()
        .unwrap();
    let expected = first.plugin.parameter_value(range.index).unwrap();
    let first = std::sync::Mutex::new(first);
    let state = snapshot_native_state(&first);
    assert!(!state.is_empty());
    first
        .lock()
        .unwrap()
        .set_parameter(parameter.id, 0.91)
        .unwrap();
    let restored = RackWorkerPlugin::load(
        &locator,
        layout,
        48_000.0,
        128,
        &crate::state::InitialState {
            parameters: serde_json::json!({}),
            native: Some(state),
        },
    )
    .unwrap();
    let actual = restored.plugin.parameter_value(range.index).unwrap();
    assert!(
        (actual - expected).abs() < 0.001,
        "native state failed to restore {expected}: {actual}"
    );
}

fn snapshot_native_state(plugin: &std::sync::Mutex<RackWorkerPlugin>) -> Vec<u8> {
    let paths = crate::worker_paths::WorkerPaths::new_in(&std::env::temp_dir())
        .expect("private worker paths");
    let shared_path = paths.shared().to_path_buf();
    let config_path = paths.config().to_path_buf();
    let (audio, control) = crate::shared_transport::create_owned(
        &paths,
        crate::AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 128,
        },
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    drop(audio);
    let control = crate::HostedProcessorControl::native(
        control,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    );
    let (_, shared) = crate::shared_transport::worker(&shared_path).unwrap();
    shared
        .header()
        .worker_status
        .store(crate::WORKER_READY, std::sync::atomic::Ordering::Release);
    let client =
        std::thread::spawn(move || control.snapshot_state(std::time::Duration::from_secs(2)));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !client.is_finished() {
        assert!(std::time::Instant::now() < deadline);
        crate::worker::broker::apply_pending_state_request(&shared, plugin);
        std::thread::yield_now();
    }
    let state = client.join().unwrap().unwrap();
    drop(shared);
    crate::worker_process::cleanup_worker_paths(&shared_path, &config_path);
    assert!(!shared_path.exists());
    state
}

#[test]
fn apple_bandpass_editor_capability_reaches_catalog_and_worker() {
    let record = adapters::probe(system_audio_unit("AUBandpass")).unwrap();
    assert!(record.descriptor.has_editor);
    assert!(record.locator.has_editor);
    let worker = RackWorkerPlugin::load(
        &record.locator,
        record
            .descriptor
            .layouts
            .iter()
            .find(|layout| layout.layout_id == record.descriptor.default_layout_id)
            .unwrap(),
        48_000.0,
        128,
        &default_state(),
    )
    .unwrap();
    assert!(worker.has_editor());
}
