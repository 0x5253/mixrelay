use super::{ControlProxyClient, ControlProxyResponse, Method};
use crate::native_text::{
    Locale,
    dialogs::{Text, text},
    resolve_locale,
};
use serde::Deserialize;
use std::{path::PathBuf, sync::Mutex};
use tauri::Manager;

#[derive(Default)]
pub(crate) struct WorkspaceFiles(Mutex<Option<PathBuf>>);

impl WorkspaceFiles {
    pub(crate) fn current(&self) -> Result<Option<PathBuf>, String> {
        self.0
            .lock()
            .map(|path| path.clone())
            .map_err(|_| "workspace path lock poisoned".into())
    }

    fn remember(&self, action: FileAction, payload: &serde_json::Value) -> Result<(), String> {
        let path = if action == FileAction::New {
            None
        } else {
            Some(
                serde_json::from_value(payload["operation"]["path"].clone())
                    .map_err(|_| "invalid workspace path")?,
            )
        };
        *self.0.lock().map_err(|_| "workspace path lock poisoned")? = path;
        Ok(())
    }
}

#[derive(Clone, Copy, Deserialize, serde::Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FileAction {
    New,
    Open,
    Save,
    SaveAs,
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
    let connection = client
        .background
        .lock()
        .map_err(|_| "background connection lock poisoned")?
        .clone();
    let locale = resolve_locale(&request.locale);
    if matches!(request.action, FileAction::New | FileAction::Open)
        && !confirm_replace(&app, locale).await
    {
        return Ok(None);
    }
    let current = if let Some(connection) = connection {
        let value = connection
            .invoke("workspace_path", serde_json::Value::Null)
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
    let operation = match request.action {
        FileAction::New => serde_json::json!({"action":"new"}),
        FileAction::Open => serde_json::json!({"action":"open", "path":selected}),
        FileAction::Save | FileAction::SaveAs => {
            serde_json::json!({"action":"save", "path":selected})
        }
    };
    let payload = serde_json::json!({
        "expectedRevision":request.expected_revision, "expectedCatalogRevision":request.expected_catalog_revision, "operation":operation
    });
    let response = submit(&app, &client, request.action, payload).await?;
    if response.status >= 200 && response.status < 300 {
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
    payload: serde_json::Value,
) -> Result<ControlProxyResponse, String> {
    let background = client
        .background
        .lock()
        .map_err(|_| "background connection lock poisoned")?
        .clone();
    if let Some(connection) = background {
        let value = connection
            .invoke(
                "workspace_submit",
                serde_json::json!({"action":action,"payload":payload}),
            )
            .await
            .map_err(|error| error.to_string())?;
        return serde_json::from_value(value).map_err(|error| error.to_string());
    }
    let response = submit_local(app, client, action, payload.clone()).await?;
    if (200..300).contains(&response.status) {
        app.state::<WorkspaceFiles>().remember(action, &payload)?;
    }
    Ok(response)
}

async fn submit_local(
    app: &tauri::AppHandle,
    client: &ControlProxyClient,
    action: FileAction,
    payload: serde_json::Value,
) -> Result<ControlProxyResponse, String> {
    #[cfg(target_os = "macos")]
    if matches!(action, FileAction::New | FileAction::Open) {
        let _guard = client.capture_gate.lock().await;
        let captures = app.state::<std::sync::Arc<crate::macos_audio::ApplicationCaptureState>>();
        return super::application_capture::workspace(client, captures.inner(), payload).await;
    }
    #[cfg(target_os = "linux")]
    let _ = (app, action);
    client
        .request(Method::POST, "/api/v1/workspace", Some(payload))
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
    if action == FileAction::New {
        return Ok(None);
    }
    if action == FileAction::Save && current.is_some() {
        return Ok(current);
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
