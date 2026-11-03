use crate::native_text::{
    current_locale,
    dialogs::{Text, text},
};
use tauri::Manager as _;

/// Resolves an existing user folder without requiring Linux XDG Desktop setup.
/// Preserves the Desktop default where present; no directory is created here.
/// Returns an error if neither the desktop nor home is an existing directory.
pub(crate) fn default_recording_folder<R: tauri::Runtime>(
    app: &impl tauri::Manager<R>,
) -> Result<std::path::PathBuf, String> {
    choose_default_folder(app.path().desktop_dir().ok(), || {
        app.path().home_dir().map_err(|error| error.to_string())
    })
}

fn choose_default_folder(
    desktop: Option<std::path::PathBuf>,
    home: impl FnOnce() -> Result<std::path::PathBuf, String>,
) -> Result<std::path::PathBuf, String> {
    if let Some(folder) = desktop.filter(|path| path.is_absolute() && path.is_dir()) {
        return Ok(folder);
    }
    let folder = home()?;
    if !folder.is_absolute() || !folder.is_dir() {
        return Err(text(current_locale(), Text::RecordingFolderUnavailable).into());
    }
    Ok(folder)
}

/// The native chooser selects only a folder; creating a take remains the recorder's job.
#[tauri::command]
pub(crate) async fn select_recording_folder(
    app: tauri::AppHandle,
    default_path: Option<String>,
    title: Option<String>,
) -> Result<Option<String>, String> {
    let directory = default_path.map_or_else(
        || default_recording_folder(&app),
        |path| Ok(std::path::PathBuf::from(path)),
    )?;
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title(
            title
                .as_deref()
                .unwrap_or_else(|| text(current_locale(), Text::RecordingFolder)),
        )
        .set_directory(directory);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog
        .pick_folder()
        .await
        .map(|folder| {
            folder
                .path()
                .to_path_buf()
                .into_os_string()
                .into_string()
                .map_err(|_| text(current_locale(), Text::RecordingFolderUnicode).into())
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::choose_default_folder;

    #[test]
    fn available_desktop_is_preserved_without_resolving_home() {
        let directory = tempfile::tempdir().unwrap();
        let selected = choose_default_folder(Some(directory.path().to_path_buf()), || {
            panic!("home must not be needed when desktop exists")
        })
        .unwrap();
        assert_eq!(selected, directory.path());
    }

    #[test]
    fn missing_desktop_uses_existing_home_and_rejects_unusable_home() {
        let directory = tempfile::tempdir().unwrap();
        for desktop in [None, Some(directory.path().join("absent"))] {
            assert_eq!(
                choose_default_folder(desktop, || Ok(directory.path().to_path_buf())).unwrap(),
                directory.path()
            );
        }
        assert!(choose_default_folder(None, || Err("home unavailable".into())).is_err());
        assert!(choose_default_folder(None, || Ok(directory.path().join("absent"))).is_err());
        assert!(choose_default_folder(None, || Ok("relative".into())).is_err());
    }
}
