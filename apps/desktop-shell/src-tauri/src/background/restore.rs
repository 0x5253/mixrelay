//! A fresh controller restores what it owns: installed Driver devices and app captures.
use std::time::Duration;

use tauri::Manager as _;

/// About thirty seconds for a started or replaced engine to answer catalog reads.
const CAPTURE_RESTORE_ATTEMPTS: u32 = 30;
const CAPTURE_RESTORE_INTERVAL: Duration = Duration::from_secs(1);

/// A fresh controller activation restores installed devices once. Reopening the
/// editor never reapplies Driver state or changes an existing graph.
pub(crate) fn restore_driver(app: &tauri::AppHandle) {
    if crate::driver_control::installed_driver_version().is_none() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match activate_driver(&app).await {
            Ok(Some(snapshot)) => {
                if let Some(menu) = app.try_state::<crate::status_menu::StatusMenu>() {
                    menu.observe_snapshot(&snapshot);
                }
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, "Driver activation unavailable; physical audio remains available");
            }
        }
    });
}

#[cfg(target_os = "macos")]
async fn activate_driver(
    app: &tauri::AppHandle,
) -> Result<Option<mixrelay_driver_core::DriverSnapshot>, String> {
    app.state::<crate::engine_lifecycle::EngineLifecycle>()
        .restore_driver(mixrelay_driver_adapters::DriverClient::session)
        .await
}

#[cfg(target_os = "linux")]
async fn activate_driver(
    _app: &tauri::AppHandle,
) -> Result<Option<mixrelay_driver_core::DriverSnapshot>, String> {
    match mixrelay_driver_adapters::DriverClient::session() {
        Ok(client) => client
            .set_active(true)
            .await
            .map(Some)
            .map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

/// The controller restores capture ownership even when no editor is open, at startup and
/// after a lost engine is replaced. Reading the catalog rebinds captures once the engine
/// answers.
pub(crate) fn restore_captures(app: &tauri::AppHandle) {
    let controller = app.clone();
    tauri::async_runtime::spawn(async move {
        mixrelay_desktop_core::lifecycle::retry(
            &crate::async_time::TokioDelay::new(mixrelay_time::StdClock::new()),
            CAPTURE_RESTORE_ATTEMPTS,
            CAPTURE_RESTORE_INTERVAL,
            || async {
                super::dispatch(
                    &controller,
                    "control_get_graph_catalog",
                    serde_json::json!({}),
                )
                .await
                .is_ok_and(|result| result["status"] == 200)
            },
        )
        .await;
    });
}
