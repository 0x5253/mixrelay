//! Driver readback and device setting changes started from the status menu.
use mixrelay_desktop_core::devices::{self, SettingAction};
use mixrelay_driver_core::DriverSnapshot;
use tauri::{AppHandle, Manager as _};

use super::{StatusMenu, playback};

/// Rereads engine power and the Driver snapshot unless another request is in flight.
pub(super) fn refresh(app: &AppHandle) {
    if !begin(app) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        playback::refresh(&app).await;
        finish(&app, crate::driver_control::driver_status().await);
    });
}

/// Applies one menu choice against fresh readback at the revision it was offered for.
pub(super) fn apply(app: &AppHandle, action: SettingAction) {
    if !begin(app) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = change(&app, action).await;
        finish(&app, result);
    });
}

fn begin(app: &AppHandle) -> bool {
    let menu = app.state::<StatusMenu>();
    let began = menu
        .lock_state()
        .is_some_and(|mut state| state.begin_driver_request());
    if began {
        let _ = menu.refresh_menu();
    }
    began
}

fn finish(app: &AppHandle, result: Result<DriverSnapshot, String>) {
    let menu = app.state::<StatusMenu>();
    if let Some(mut state) = menu.lock_state() {
        state.finish_driver_request(result);
    }
    let _ = menu.refresh_menu();
}

/// A rejected or failed change still shows the Driver's current readback.
async fn change(app: &AppHandle, action: SettingAction) -> Result<DriverSnapshot, String> {
    let snapshot = crate::driver_control::driver_status().await?;
    let command = match devices::command(&snapshot, &action) {
        Ok(command) => command,
        Err(error) => {
            show_readback(app, snapshot);
            return Err(error.to_string());
        }
    };
    let result = crate::driver_control::driver_mutate(app.clone(), command).await;
    if result.is_err()
        && let Ok(snapshot) = crate::driver_control::driver_status().await
    {
        show_readback(app, snapshot);
    }
    result
}

fn show_readback(app: &AppHandle, snapshot: DriverSnapshot) {
    if let Some(mut state) = app.state::<StatusMenu>().lock_state() {
        state.snapshot = Some(snapshot);
    }
}
