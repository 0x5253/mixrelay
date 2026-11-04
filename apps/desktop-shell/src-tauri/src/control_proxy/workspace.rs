//! Workspace file commands: confirmation and file dialogs here, the submission itself
//! through the background runtime or the engine.
use mixrelay_desktop_core::engine_control::routes;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};
use tauri::Manager;

use super::{ControlProxyClient, ControlProxyResponse, Method};
use crate::native_text::{
    Locale,
    dialogs::{Text, text},
    resolve_locale,
};

#[derive(Default)]
pub(crate) struct WorkspaceFiles(Mutex<Option<PathBuf>>);

impl WorkspaceFiles {
    pub(crate) fn current(&self) -> Result<Option<PathBuf>, String> {
        self.0
            .lock()
            .map(|path| path.clone())
            .map_err(|_| "workspace path lock poisoned".into())
    }

    fn remember(&self, action: FileAction, payload: &Value) -> Result<(), String> {
        let path = action.resulting_path(payload)?;
        *self.0.lock().map_err(|_| "workspace path lock poisoned")? = path;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FileAction {
    New,
    Open,
    Save,
    SaveAs,
}

impl FileAction {
    /// New and Open discard the current workspace, so the user confirms them first.
    fn replaces_workspace(self) -> bool {
        matches!(self, Self::New | Self::Open)
    }

    /// Where this action's file comes from, given the workspace's current file.
    fn file(self, current: Option<&Path>) -> FileSource {
        match (self, current) {
            (Self::New, _) => FileSource::None,
            (Self::Save, Some(path)) => FileSource::Current(path.to_owned()),
            (Self::Save, None) | (Self::Open | Self::SaveAs, _) => FileSource::Dialog,
        }
    }

    /// The engine's workspace operation for this action on `path`.
    fn operation(self, path: Option<&Path>) -> Value {
        match self {
            Self::New => json!({"action":"new"}),
            Self::Open => json!({"action":"open", "path":path}),
            Self::Save | Self::SaveAs => json!({"action":"save", "path":path}),
        }
    }

    /// The workspace file once this action's `payload` was applied.
    fn resulting_path(self, payload: &Value) -> Result<Option<PathBuf>, String> {
        if self == Self::New {
            return Ok(None);
        }
        serde_json::from_value(payload["operation"]["path"].clone())
            .map(Some)
            .map_err(|_| "invalid workspace path".to_owned())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum FileSource {
    /// A new workspace has no file until it is saved.
    None,
    /// Save writes back to the file the workspace came from.
    Current(PathBuf),
    /// Open, Save As, and a first Save ask the user.
    Dialog,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FileRequest {
    action: FileAction,
    expected_revision: u64,
    expected_catalog_revision: u64,
    locale: String,
}

#[tauri::command]
pub(crate) async fn workspace_file(
    app: tauri::AppHandle,
    client: tauri::State<'_, ControlProxyClient>,
    files: tauri::State<'_, WorkspaceFiles>,
    request: FileRequest,
) -> Result<Option<ControlProxyResponse>, String> {
    let connection = client.background_connection()?;
    let locale = resolve_locale(&request.locale);
    if request.action.replaces_workspace() && !confirm_replace(&app, locale).await {
        return Ok(None);
    }
    let current = if let Some(connection) = connection {
        let value = connection
            .invoke("workspace_path", Value::Null)
            .await
            .map_err(|error| error.to_string())?;
        serde_json::from_value(value).map_err(|error| error.to_string())?
    } else {
        files.current()?
    };
    let selected = select_path(&app, request.action, current, locale).await?;
    if selected.is_none() && request.action != FileAction::New {
        return Ok(None);
    }
    let payload = json!({
        "expectedRevision": request.expected_revision,
        "expectedCatalogRevision": request.expected_catalog_revision,
        "operation": request.action.operation(selected.as_deref()),
    });
    let response = submit(&app, &client, request.action, payload).await?;
    if response.is_success() {
        *files
            .0
            .lock()
            .map_err(|_| text(locale, Text::WorkspaceLockFailed))? = selected;
    }
    Ok(Some(response))
}

pub(crate) async fn submit(
    app: &tauri::AppHandle,
    client: &ControlProxyClient,
    action: FileAction,
    payload: Value,
) -> Result<ControlProxyResponse, String> {
    if let Some(connection) = client.background_connection()? {
        let value = connection
            .invoke(
                "workspace_submit",
                json!({"action":action,"payload":payload}),
            )
            .await
            .map_err(|error| error.to_string())?;
        return serde_json::from_value(value).map_err(|error| error.to_string());
    }
    let response = submit_local(app, client, action, payload.clone()).await?;
    if response.is_success() {
        app.state::<WorkspaceFiles>().remember(action, &payload)?;
    }
    Ok(response)
}

async fn submit_local(
    app: &tauri::AppHandle,
    client: &ControlProxyClient,
    action: FileAction,
    payload: Value,
) -> Result<ControlProxyResponse, String> {
    // Replacing the workspace can change which applications the engine captures.
    #[cfg(target_os = "macos")]
    if action.replaces_workspace() {
        let _guard = client.capture_gate.lock().await;
        let captures = app.state::<std::sync::Arc<crate::macos_audio::ApplicationCaptureState>>();
        return super::application_capture::workspace(client, captures.inner(), payload).await;
    }
    #[cfg(target_os = "linux")]
    let _ = (app, action);
    client
        .request(Method::POST, routes::WORKSPACE, Some(payload))
        .await
}

async fn confirm_replace(app: &tauri::AppHandle, locale: Locale) -> bool {
    let Some(window) = app.get_webview_window("main") else {
        return false;
    };
    rfd::AsyncMessageDialog::new()
        .set_parent(&window)
        .set_title("MixRelay")
        .set_description(text(locale, Text::WorkspaceReplacePrompt))
        .set_buttons(rfd::MessageButtons::OkCancelCustom(
            text(locale, Text::WorkspaceReplace).into(),
            text(locale, Text::Cancel).into(),
        ))
        .show()
        .await
        == rfd::MessageDialogResult::Custom(text(locale, Text::WorkspaceReplace).into())
}

async fn select_path(
    app: &tauri::AppHandle,
    action: FileAction,
    current: Option<PathBuf>,
    locale: Locale,
) -> Result<Option<PathBuf>, String> {
    match action.file(current.as_deref()) {
        FileSource::None => return Ok(None),
        FileSource::Current(path) => return Ok(Some(path)),
        FileSource::Dialog => {}
    }
    let directory = app
        .path()
        .home_dir()
        .map_err(|error| error.to_string())?
        .join("mixrelay");
    let create = directory.clone();
    tauri::async_runtime::spawn_blocking(move || std::fs::create_dir_all(create))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    let dialog = rfd::AsyncFileDialog::new()
        .set_parent(
            &app.get_webview_window("main")
                .ok_or_else(|| text(locale, Text::MainWindowUnavailable))?,
        )
        .add_filter("MixRelay", &["mix"])
        .set_directory(
            current
                .as_ref()
                .and_then(|path| path.parent())
                .unwrap_or(&directory),
        )
        .set_file_name(
            current
                .as_ref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("Untitled.mix"),
        );
    let path = if action == FileAction::Open {
        dialog
            .set_title(text(locale, Text::WorkspaceOpen))
            .pick_file()
            .await
    } else {
        dialog
            .set_title(text(locale, Text::WorkspaceSave))
            .save_file()
            .await
    };
    Ok(path.map(|file| file.path().to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIONS: [FileAction; 4] = [
        FileAction::New,
        FileAction::Open,
        FileAction::Save,
        FileAction::SaveAs,
    ];

    #[test]
    fn only_actions_that_discard_the_workspace_ask_for_confirmation() {
        let confirmed: Vec<_> = ACTIONS
            .into_iter()
            .filter(|action| action.replaces_workspace())
            .collect();
        assert_eq!(confirmed, [FileAction::New, FileAction::Open]);
    }

    #[test]
    fn save_reuses_the_current_file_while_open_and_save_as_always_ask() {
        let current = Path::new("/work/set.mix");
        assert_eq!(FileAction::New.file(Some(current)), FileSource::None);
        assert_eq!(FileAction::New.file(None), FileSource::None);
        assert_eq!(
            FileAction::Save.file(Some(current)),
            FileSource::Current(current.to_owned())
        );
        assert_eq!(FileAction::Save.file(None), FileSource::Dialog);
        for action in [FileAction::Open, FileAction::SaveAs] {
            assert_eq!(action.file(Some(current)), FileSource::Dialog);
            assert_eq!(action.file(None), FileSource::Dialog);
        }
    }

    #[test]
    fn operations_name_the_engine_action_and_carry_the_chosen_file() {
        let path = Path::new("/work/set.mix");
        assert_eq!(FileAction::New.operation(None), json!({"action":"new"}));
        assert_eq!(
            FileAction::Open.operation(Some(path)),
            json!({"action":"open","path":"/work/set.mix"})
        );
        for action in [FileAction::Save, FileAction::SaveAs] {
            assert_eq!(
                action.operation(Some(path)),
                json!({"action":"save","path":"/work/set.mix"})
            );
        }
    }

    #[test]
    fn a_successful_submission_remembers_its_file_and_new_forgets_it() {
        let files = WorkspaceFiles::default();
        let saved = json!({"operation": FileAction::SaveAs.operation(Some(Path::new("/a.mix")))});
        files.remember(FileAction::SaveAs, &saved).unwrap();
        assert_eq!(files.current().unwrap(), Some(PathBuf::from("/a.mix")));
        let new = json!({"operation": FileAction::New.operation(None)});
        files.remember(FileAction::New, &new).unwrap();
        assert_eq!(files.current().unwrap(), None);
        let pathless = json!({"operation":{"action":"open"}});
        assert!(files.remember(FileAction::Open, &pathless).is_err());
    }
}
