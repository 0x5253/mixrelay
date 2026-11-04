//! Window and application events: closing hides the editor, reopening shows it, and Quit
//! waits for the engine.
use tauri::Manager as _;

use crate::app_mode::AppMode;

pub(crate) fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event
        && window.label() == "main"
    {
        api.prevent_close();
        let app = window.app_handle().clone();
        tauri::async_runtime::spawn(async move { close_main_window(&app).await });
    }
}

async fn close_main_window(app: &tauri::AppHandle) {
    crate::midi::cancel_editor_learn(app).await;
    if app.state::<AppMode>().quits_on_main_window_close() {
        app.exit(0);
        return;
    }
    if let Err(error) = crate::app_menu::hide_main_window(app) {
        tracing::error!(%error, "could not hide main window");
    }
}

pub(crate) fn on_run_event(app: &tauri::AppHandle, event: &tauri::RunEvent) {
    match event {
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Exit => crate::native_termination::release(),
        tauri::RunEvent::ExitRequested { api, .. } => {
            if let Some(lifecycle) = app.try_state::<crate::engine_lifecycle::EngineLifecycle>() {
                lifecycle.exit_requested(app, api);
            }
        }
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => reopen(app),
        _ => {}
    }
}

#[cfg(target_os = "macos")]
fn reopen(app: &tauri::AppHandle) {
    match *app.state::<AppMode>() {
        AppMode::Service => {
            // With no editor running, reopening MixRelay targets this process, and
            // LaunchServices promotes it to a Dock app because the shared bundle is a
            // regular app. The runtime stays in the background; the editor it opens is
            // the one Dock app.
            if let Err(error) = app.set_activation_policy(tauri::ActivationPolicy::Accessory) {
                tracing::error!(%error, "could not keep the runtime in the background");
            }
            crate::status_menu::open_editor();
        }
        AppMode::Graph => reopen_editor(app),
    }
}

/// Showing the editor never reconnects; its visible startup gate owns explicit reconnects,
/// so background polling never restarts stopped audio.
pub(crate) fn reopen_editor(app: &tauri::AppHandle) {
    if let Err(error) = crate::app_menu::show_main_window(app) {
        tracing::error!(%error, "could not reopen the editor");
    }
}
