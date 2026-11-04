//! Quit waits for confirmed engine teardown; a failed teardown keeps the app running so
//! Quit can be retried.
use std::{future::Future, sync::Arc};

use mixrelay_desktop_core::lifecycle::shutdown::{ExitRequest, ShutdownGate};

use crate::native_text::{
    current_locale,
    dialogs::{Text, text},
};

/// Holds the exit until `teardown` confirms it. `begin` runs once per teardown attempt,
/// before it starts, and returns the attempt; its result is a warning to show before exit.
pub(crate) fn on_exit_requested<T>(
    app: &tauri::AppHandle,
    api: &tauri::ExitRequestApi,
    gate: &Arc<ShutdownGate>,
    begin: impl FnOnce() -> T,
    failure: &'static str,
) where
    T: Future<Output = Result<Option<String>, String>> + Send + 'static,
{
    match gate.request_exit() {
        ExitRequest::Allow => return,
        ExitRequest::Hold => {
            api.prevent_exit();
            return;
        }
        ExitRequest::Begin => api.prevent_exit(),
    }
    let teardown = begin();
    let gate = Arc::clone(gate);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match teardown.await {
            Ok(warning) => {
                if let Some(warning) = warning {
                    show_warning(&warning).await;
                }
                gate.complete();
                app.exit(0);
            }
            Err(error) => {
                gate.attempt_failed();
                tracing::error!(%error, "{failure}");
                crate::status_menu::shutdown_failed(&app).await;
            }
        }
    });
}

async fn show_warning(warning: &str) {
    rfd::AsyncMessageDialog::new()
        .set_title("MixRelay")
        .set_description(warning)
        .set_level(rfd::MessageLevel::Error)
        .set_buttons(rfd::MessageButtons::OkCustom(
            text(current_locale(), Text::Ok).into(),
        ))
        .show()
        .await;
}
