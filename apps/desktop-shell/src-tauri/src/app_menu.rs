//! Main window visibility and the native menus' locale.
use tauri::Manager;

use crate::workspace_menu::WorkspaceMenu;

/// Hides the main window without stopping audio or removing its Dock presence.
/// Returns lookup, window or activation-policy errors without requesting exit.
pub(crate) fn hide_main_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    let window = app
        .get_webview_window("main")
        .ok_or(tauri::Error::WindowNotFound)?;
    window.hide()?;
    Ok(())
}

/// Restores the main window and its Dock presence, returning native window errors.
pub(crate) fn show_main_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    let window = app
        .get_webview_window("main")
        .ok_or(tauri::Error::WindowNotFound)?;
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Regular)?;
    window.unminimize()?;
    window.show()?;
    window.set_focus()
}

/// Applies the UI's resolved locale to its role's native menu.
///
/// Returns a native error when an item label cannot be updated.
#[tauri::command]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; a borrowed State is not a command argument"
)]
pub(crate) async fn set_native_menu_locale(
    locale: &str,
    app: tauri::AppHandle,
) -> Result<(), String> {
    crate::native_text::set_locale(locale);
    if let Some(menu) = app.try_state::<crate::status_menu::StatusMenu>() {
        return menu.set_locale(locale).map_err(|error| error.to_string());
    }
    app.state::<WorkspaceMenu>()
        .set_locale(locale)
        .map_err(|error| error.to_string())?;
    let connection = app
        .try_state::<crate::background::Connection>()
        .map(|state| state.inner().clone());
    if let Some(connection) = connection {
        connection
            .invoke(
                "set_native_menu_locale",
                serde_json::json!({"locale":locale}),
            )
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
