//! Initializes the isolated native instance and joins DSP before creator-thread destruction.

use crate::{
    EDITOR_FAILURE_NO_EDITOR, EDITOR_STATE_CLOSED, EDITOR_STATE_UNAVAILABLE, RackLocator,
    WORKER_FAILED, WORKER_READY, dsp_lifecycle, latency, rack_formats,
    shared_transport::{self, SharedHeader},
    worker_configuration,
};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

pub(crate) mod broker;
pub(crate) mod dsp;

// Establish agent status before a vendor loads AppKit or creates an editor. Setting it
// only when our editor host starts is too late for discovery and instance creation.
#[cfg_attr(
    not(target_os = "macos"),
    expect(
        clippy::unnecessary_wraps,
        reason = "one process-preparation contract; AppKit setup can fail on macOS"
    )
)]
pub(super) fn prepare_plugin_process() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        let main =
            MainThreadMarker::new().ok_or("plug-in process must start on the main thread")?;
        let app = NSApplication::sharedApplication(main);
        if !app.setActivationPolicy(NSApplicationActivationPolicy::Accessory) {
            return Err("could not initialize background plug-in process".into());
        }
        app.finishLaunching();
    }
    Ok(())
}

/// Runs one `AU`, `AUv3`, `VST3`, `CLAP`, or `LV2` instance in the common isolated DSP worker.
///
/// # Errors
///
/// Returns a diagnostic when the target identity, shared transport, or plug-in cannot initialize.
pub fn run_plugin_worker(arguments: &[String]) -> Result<(), String> {
    let [encoded_locator, shared_path, config_path] = arguments else {
        return Err(
            "processor worker requires locator, shared-memory, and config paths".to_owned(),
        );
    };
    let shutdown = Arc::new(AtomicBool::new(false));
    let _stdin_guard =
        dsp_lifecycle::watch_parent(Arc::clone(&shutdown), dsp_lifecycle::PARENT_GRACE)
            .map_err(|error| error.to_string())?;
    prepare_plugin_process()?;
    let locator = RackLocator::decode(encoded_locator)?;
    let (mut audio_shared, editor_shared) = shared_transport::worker(Path::new(shared_path))?;
    let editor_shared = Arc::new(editor_shared);
    let layout = audio_shared.layout();
    let prepared = worker_configuration::read_worker_configuration(config_path)?;
    let plugin = {
        let _operation = editor_shared
            .header()
            .native_operation
            .enter(crate::native_operation::Stage::Load);
        rack_formats::RackWorkerPlugin::load(
            &locator,
            &prepared.ports,
            prepared.sample_rate,
            layout.block_frames,
            &prepared.initial,
        )?
    };
    if plugin.input_channels() != layout.inputs || plugin.output_channels() != layout.outputs {
        return Err("loaded processor layout does not match the catalog".to_owned());
    }
    let prepared_rate = prepared.sample_rate;
    let has_editor = plugin.has_editor();
    latency::publish_intrinsic_frames(editor_shared.header(), plugin.intrinsic_latency_frames());
    let plugin = Arc::new(Mutex::new(plugin));
    let dsp_plugin = Arc::clone(&plugin);
    let dsp_shutdown = Arc::clone(&shutdown);
    let dsp_thread = thread::Builder::new()
        .name("mixrelay-plugin-dsp".to_owned())
        .spawn(move || {
            dsp::run(
                &mut audio_shared,
                &dsp_plugin,
                layout,
                &dsp_shutdown,
                prepared_rate,
            );
        })
        .map_err(|error| error.to_string())?;
    // Retain the creator's instance owner until DSP has joined, including broker unwinding.
    let dsp_owner =
        dsp_lifecycle::DspThreadOwner::new(dsp_thread, Arc::clone(&shutdown), Arc::clone(&plugin));
    let editor_header = editor_shared.header();
    editor_header.editor_state.store(
        if has_editor {
            EDITOR_STATE_CLOSED
        } else {
            EDITOR_STATE_UNAVAILABLE
        },
        Ordering::Release,
    );
    if !has_editor {
        editor_header
            .editor_failure
            .store(EDITOR_FAILURE_NO_EDITOR, Ordering::Release);
    }
    editor_header
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    let result = broker::run(&editor_shared, &plugin, locator.display_name(), &shutdown);
    let _operation = editor_shared
        .header()
        .native_operation
        .enter(crate::native_operation::Stage::Destroy);
    let result = dsp_owner.finish(result);
    drop(plugin);
    result
}

fn mark_plugin_worker_failed(header: &SharedHeader) {
    latency::publish_intrinsic_frames(header, None);
    header.worker_status.store(WORKER_FAILED, Ordering::Release);
}
