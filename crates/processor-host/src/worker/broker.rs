//! Creator-thread native control, state export and editor-window lifecycle.

use super::mark_plugin_worker_failed;
use crate::{
    EDITOR_COMMAND_CLOSE, EDITOR_COMMAND_OPEN, EDITOR_FAILURE_DISPLAY_UNAVAILABLE,
    EDITOR_FAILURE_HOST_WINDOW, EDITOR_FAILURE_NO_EDITOR, EDITOR_FAILURE_NONE,
    EDITOR_FAILURE_PLUGIN_REJECTED, EDITOR_FAILURE_WORKER_UNAVAILABLE, EDITOR_STATE_CLOSED,
    EDITOR_STATE_FAILED, EDITOR_STATE_OPEN, PARAMETER_CAPACITY, ProcessorStateError,
    editor_surface, latency, native_editor, rack_formats,
    shared_transport::{SharedHeader, WorkerControl},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub(super) fn run(
    shared: &Arc<WorkerControl>,
    plugin: &Mutex<rack_formats::RackWorkerPlugin>,
    title: &str,
    parent_closed: &AtomicBool,
) -> Result<(), String> {
    let mut host: Option<native_editor::NativeEditorHost> = None;
    let mut last_idle = Instant::now();
    let mut result = Ok(());
    loop {
        let header = shared.header();
        if parent_closed.load(Ordering::Acquire) || header.shutdown.load(Ordering::Acquire) != 0 {
            if let (Some(host), Ok(mut plugin)) = (host.as_mut(), plugin.lock()) {
                // Service failure stops DSP, but the initialized native object still owns its
                // GUI. Close it on its main thread before removing the parent window or instance.
                let _operation = header
                    .native_operation
                    .enter(crate::native_operation::Stage::Editor);
                host.close(&mut plugin);
            }
            return result;
        }
        if let Err(error) = service_plugin_control(shared, plugin) {
            result = Err(error);
            continue;
        }
        apply_pending_editor_command(shared, plugin, title, &mut host);
        // Native state APIs may allocate, block, or require the UI thread. They run only on
        // this broker, never in the engine callback or the worker's DSP processing loop.
        apply_pending_state_request(shared, plugin);
        if last_idle.elapsed() >= Duration::from_millis(16) {
            tick_editor(shared, plugin, &mut host);
            last_idle = Instant::now();
        }
        thread::sleep(Duration::from_millis(2));
    }
}

pub(crate) fn service_plugin_control(
    shared: &WorkerControl,
    plugin: &Mutex<rack_formats::RackWorkerPlugin>,
) -> Result<(), String> {
    let header = shared.header();
    let mut plugin = match plugin.try_lock() {
        Ok(plugin) => plugin,
        Err(std::sync::TryLockError::WouldBlock) => return Ok(()),
        Err(std::sync::TryLockError::Poisoned(_)) => {
            mark_plugin_worker_failed(header);
            header.shutdown.store(1, Ordering::Release);
            return Err("processor instance lock was poisoned".to_owned());
        }
    };
    let result = {
        let _operation = header
            .native_operation
            .enter(crate::native_operation::Stage::Service);
        plugin.service_main_thread()
    };
    if let Err(error) = result {
        // Publish failure before releasing the lock so no DSP call can race a failed restart.
        mark_plugin_worker_failed(header);
        header.shutdown.store(1, Ordering::Release);
        return Err(format!("processor main-thread service failed: {error}"));
    }
    // Native parameter controllers belong to the instance's creator thread. DSP only
    // consumes the prepared format event queues on its next real audio block. Servicing
    // the mailbox here also applies edits while no PCM or editor window is present.
    apply_pending_rack_parameter(header, &mut plugin);
    Ok(())
}

pub(crate) fn apply_pending_state_request(
    shared: &WorkerControl,
    plugin: &Mutex<rack_formats::RackWorkerPlugin>,
) {
    let header = shared.header();
    let Some((request, kind)) = shared.pending_request() else {
        return;
    };
    if shared.request_cancelled(request) {
        // A deferred native edit must not leave a timed-out reader's request unacknowledged.
        shared.publish(request, Err(ProcessorStateError::Unavailable));
        return;
    }
    if kind == editor_surface::EDITOR_FRAME {
        if !header.editor_observed_open.load(Ordering::Acquire) {
            shared.publish(request, Err(ProcessorStateError::Unavailable));
        }
        return;
    }
    let mut plugin = match plugin.try_lock() {
        Ok(plugin) => plugin,
        Err(std::sync::TryLockError::WouldBlock) => return,
        Err(std::sync::TryLockError::Poisoned(_)) => {
            mark_plugin_worker_failed(header);
            return;
        }
    };
    // Deliver accepted controls to the adapter before capture. Some formats queue native
    // events until the next real audio block; release the lock and retry after DSP in that
    // case, within the reader's existing timeout, without manufacturing an OFF-node block.
    apply_pending_rack_parameter(shared.header(), &mut plugin);
    let state = {
        let _operation = header
            .native_operation
            .enter(crate::native_operation::Stage::State);
        plugin.save_state()
    };
    if matches!(state, Err(ProcessorStateError::Pending)) {
        return;
    }
    latency::publish_intrinsic_frames(header, plugin.intrinsic_latency_frames());
    // Native capture needs the instance lock; transporting the completed bytes does not.
    // Release it before waiting for the control reader so DSP can resume immediately.
    drop(plugin);
    shared.publish(request, state);
}

fn apply_pending_editor_command(
    shared: &Arc<WorkerControl>,
    plugin: &Mutex<rack_formats::RackWorkerPlugin>,
    title: &str,
    host: &mut Option<native_editor::NativeEditorHost>,
) {
    let header = shared.header();
    let sequence = header.editor_sequence.load(Ordering::Acquire);
    if sequence == header.editor_consumed.load(Ordering::Relaxed) {
        return;
    }
    let command = header.editor_command.load(Ordering::Relaxed);
    if command == EDITOR_COMMAND_CLOSE {
        let snapshot = if let Some(host) = host.as_mut() {
            let Ok(mut plugin) = plugin.lock() else {
                mark_plugin_worker_failed(header);
                publish_editor_failure(header, EDITOR_FAILURE_WORKER_UNAVAILABLE);
                return;
            };
            let _operation = header
                .native_operation
                .enter(crate::native_operation::Stage::Editor);
            host.close(&mut plugin);
            let snapshot = plugin.tick_editor();
            latency::publish_intrinsic_frames(header, plugin.intrinsic_latency_frames());
            host.refresh(snapshot);
            snapshot
        } else {
            rack_formats::EditorSnapshot::default()
        };
        header
            .editor_failure
            .store(EDITOR_FAILURE_NONE, Ordering::Release);
        publish_observed_editor_snapshot(header, snapshot);
    } else if command == EDITOR_COMMAND_OPEN {
        open_editor(shared, plugin, title, host);
    }
    header.editor_consumed.store(sequence, Ordering::Release);
}

fn open_editor(
    shared: &Arc<WorkerControl>,
    plugin: &Mutex<rack_formats::RackWorkerPlugin>,
    title: &str,
    host: &mut Option<native_editor::NativeEditorHost>,
) {
    let header = shared.header();
    let has_editor = if let Ok(plugin) = plugin.lock() {
        plugin.has_editor()
    } else {
        mark_plugin_worker_failed(header);
        publish_editor_failure(header, EDITOR_FAILURE_WORKER_UNAVAILABLE);
        return;
    };
    if !has_editor {
        publish_editor_failure(header, EDITOR_FAILURE_NO_EDITOR);
        return;
    }
    let host = match host {
        Some(host) => host,
        None => match native_editor::NativeEditorHost::new(Arc::clone(shared)) {
            Ok(created) => host.insert(created),
            Err(native_editor::EditorHostFailure::DisplayUnavailable) => {
                publish_editor_failure(header, EDITOR_FAILURE_DISPLAY_UNAVAILABLE);
                return;
            }
            Err(_) => {
                publish_editor_failure(header, EDITOR_FAILURE_HOST_WINDOW);
                return;
            }
        },
    };
    let result = if let Ok(mut plugin) = plugin.lock() {
        let _operation = header
            .native_operation
            .enter(crate::native_operation::Stage::Editor);
        let result = host.open(&mut plugin, title);
        latency::publish_intrinsic_frames(header, plugin.intrinsic_latency_frames());
        result
    } else {
        mark_plugin_worker_failed(header);
        Err(native_editor::EditorHostFailure::PluginRejected)
    };
    match result {
        Ok(snapshot) if snapshot.open => {
            header
                .editor_failure
                .store(EDITOR_FAILURE_NONE, Ordering::Relaxed);
            publish_observed_editor_snapshot(header, snapshot);
        }
        Ok(_) | Err(native_editor::EditorHostFailure::PluginRejected) => {
            publish_editor_failure(header, EDITOR_FAILURE_PLUGIN_REJECTED);
        }
        Err(native_editor::EditorHostFailure::DisplayUnavailable) => {
            publish_editor_failure(header, EDITOR_FAILURE_DISPLAY_UNAVAILABLE);
        }
        Err(native_editor::EditorHostFailure::HostWindow) => {
            publish_editor_failure(header, EDITOR_FAILURE_HOST_WINDOW);
        }
    }
}

fn tick_editor(
    shared: &WorkerControl,
    plugin: &Mutex<rack_formats::RackWorkerPlugin>,
    host: &mut Option<native_editor::NativeEditorHost>,
) {
    let Some(editor_host) = host.as_mut() else {
        return;
    };
    let header = shared.header();
    // The host retains close/resize events until the plug-in can consume them. Native
    // mouse tracking can run a nested event loop; pumping it under this mutex stalls DSP.
    {
        let _operation = header
            .native_operation
            .enter(crate::native_operation::Stage::EditorEvents);
        editor_host.poll_events();
    }
    let mut plugin = match plugin.try_lock() {
        Ok(plugin) => plugin,
        Err(std::sync::TryLockError::WouldBlock) => return,
        Err(std::sync::TryLockError::Poisoned(_)) => {
            mark_plugin_worker_failed(header);
            return;
        }
    };
    let _operation = header
        .native_operation
        .enter(crate::native_operation::Stage::Editor);
    let events = editor_host.take_events();
    if events.close_requested {
        editor_host.close(&mut plugin);
    } else if let Some((width, height)) = events.resized {
        let _ = plugin.resize_editor(width, height);
    }
    let snapshot = plugin.tick_editor();
    latency::publish_intrinsic_frames(header, plugin.intrinsic_latency_frames());
    editor_host.refresh(snapshot);
    publish_observed_editor_snapshot(header, snapshot);
}

pub(crate) fn publish_observed_editor_snapshot(
    header: &SharedHeader,
    snapshot: rack_formats::EditorSnapshot,
) {
    publish_editor_snapshot(header, snapshot);
    header.editor_state.store(
        if snapshot.open {
            EDITOR_STATE_OPEN
        } else if header.editor_failure.load(Ordering::Acquire) != EDITOR_FAILURE_NONE {
            // Idle native ticks are not retries and cannot clear a failed-open result.
            EDITOR_STATE_FAILED
        } else {
            EDITOR_STATE_CLOSED
        },
        Ordering::Release,
    );
    let was_open = header
        .editor_observed_open
        .swap(snapshot.open, Ordering::AcqRel);
    if was_open && !snapshot.open {
        // Only the native broker can acknowledge a close. Requests, crashes, initial closed
        // state, and repeated idle ticks cannot manufacture a durable-state checkpoint.
        header
            .editor_close_generation
            .fetch_add(1, Ordering::Release);
    }
}

fn publish_editor_snapshot(header: &SharedHeader, snapshot: rack_formats::EditorSnapshot) {
    let (width, height) = snapshot.size.unwrap_or((0, 0));
    header.editor_width.store(width, Ordering::Relaxed);
    header.editor_height.store(height, Ordering::Relaxed);
    header
        .editor_resizable
        .store(u32::from(snapshot.resizable), Ordering::Relaxed);
}

pub(crate) fn publish_editor_failure(header: &SharedHeader, failure: u32) {
    publish_editor_snapshot(header, rack_formats::EditorSnapshot::default());
    header.editor_failure.store(failure, Ordering::Relaxed);
    header
        .editor_state
        .store(EDITOR_STATE_FAILED, Ordering::Release);
}

fn apply_pending_rack_parameter(
    header: &SharedHeader,
    plugin: &mut rack_formats::RackWorkerPlugin,
) {
    consume_parameters(header, |id, value| {
        let _operation = header
            .native_operation
            .enter(crate::native_operation::Stage::Parameter);
        if plugin.set_parameter(id, value).is_err() {
            header
                .process_faults
                .record(crate::process_fault::FaultKind::ParameterApplication);
        }
    });
    latency::publish_intrinsic_frames(header, plugin.intrinsic_latency_frames());
}

pub(crate) fn consume_parameters(header: &SharedHeader, mut apply: impl FnMut(u32, f64)) {
    if !header.parameters_pending.swap(false, Ordering::AcqRel) {
        return;
    }
    let count = (header.parameter_count.load(Ordering::Acquire) as usize).min(PARAMETER_CAPACITY);
    for slot in &header.parameters[..count] {
        if slot.pending.swap(false, Ordering::AcqRel) {
            apply(
                slot.id.load(Ordering::Relaxed),
                f64::from_bits(slot.value_bits.load(Ordering::Acquire)),
            );
        }
    }
}
