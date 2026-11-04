//! Update checks and download links started from the status menu.
use std::time::Duration;

use mixrelay_desktop_core::updates::{Download, Product};
use tauri::{AppHandle, Manager as _};

use super::{StatusMenu, state::UpdateResult};

const AUTOMATIC_CHECK_INTERVAL: Duration = Duration::from_hours(24);

/// Checks once at launch and then daily. Automatic checks only surface a download.
pub(super) fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(AUTOMATIC_CHECK_INTERVAL);
        loop {
            interval.tick().await;
            check(&app, false);
        }
    });
}

/// Starts a check unless one is running; a manual check always reports its result.
pub(super) fn check(app: &AppHandle, manual: bool) {
    let menu = app.state::<StatusMenu>();
    if !menu
        .lock_state()
        .is_some_and(|mut state| state.begin_update_check())
    {
        return;
    }
    let _ = menu.refresh_menu();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let update = crate::updates::updates_check_driver().await;
        let download = update
            .latest_version()
            .zip(update.downloads().first().map(Download::url));
        let result = UpdateResult::after_check(update.state(), download, manual);
        let menu = app.state::<StatusMenu>();
        if let Some(mut state) = menu.lock_state() {
            state.finish_update_check(result);
        }
        let _ = menu.refresh_menu();
    });
}

/// Opens the offered package in the browser after rechecking the publication.
pub(super) fn open_download(app: &AppHandle) {
    let menu = app.state::<StatusMenu>();
    let Some(url) = menu
        .lock_state()
        .and_then(|mut state| state.begin_download())
    else {
        return;
    };
    let _ = menu.refresh_menu();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = crate::updates::updates_open_download(Product::MixRelay, url).await;
        if let Err(error) = &result {
            tracing::error!(%error, "update download could not be opened");
        }
        let menu = app.state::<StatusMenu>();
        if let Some(mut state) = menu.lock_state() {
            state.finish_download(result.is_ok());
        }
        let _ = menu.refresh_menu();
    });
}
