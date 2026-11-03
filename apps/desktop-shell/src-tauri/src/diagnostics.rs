//! Explicit, bounded support export. No recorder, timer, retained dump, or upload.
use mixrelay_driver_core::DriverSnapshot;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::{
    io::Write,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::Manager as _;

use mixrelay_desktop_core::diagnostics::{self, ExportPermit, ReportEnvironment};
pub(crate) use mixrelay_desktop_core::diagnostics::{EditorContext, ExportAdmission};
use std::sync::Arc;
const REPORT_LIMIT: usize = diagnostics::REPORT_LIMIT;

struct ExportGuard {
    app: tauri::AppHandle,
    permit: Option<ExportPermit>,
}
impl Drop for ExportGuard {
    fn drop(&mut self) {
        self.permit.take();
        if let Some(menu) = self.app.try_state::<crate::status_menu::StatusMenu>() {
            menu.refresh_diagnostics();
        }
    }
}
pub(crate) fn is_exporting(app: &tauri::AppHandle) -> bool {
    app.state::<Arc<ExportAdmission>>().exporting()
}

pub(crate) fn label(language: &str) -> &'static str {
    LABELS[crate::native_text::resolve_locale(language).index()].0
}

const LABELS: [(&str, &str); 11] = [
    (
        "Save Diagnostic Report…",
        "Could not save the diagnostic report. Check the destination and try again.",
    ),
    (
        "진단 리포트 저장…",
        "진단 리포트를 저장하지 못했습니다. 저장 위치를 확인하고 다시 시도하십시오.",
    ),
    (
        "診断レポートを保存…",
        "診断レポートを保存できませんでした。保存先を確認して再試行してください。",
    ),
    ("保存诊断报告…", "无法保存诊断报告。请检查保存位置并重试。"),
    ("儲存診斷報告…", "無法儲存診斷報告。請檢查儲存位置並重試。"),
    (
        "Enregistrer un rapport de diagnostic…",
        "Impossible d’enregistrer le rapport de diagnostic. Vérifiez la destination et réessayez.",
    ),
    (
        "Diagnosebericht speichern…",
        "Der Diagnosebericht konnte nicht gespeichert werden. Prüfen Sie den Speicherort und versuchen Sie es erneut.",
    ),
    (
        "Guardar informe de diagnóstico…",
        "No se pudo guardar el informe de diagnóstico. Compruebe el destino e inténtelo de nuevo.",
    ),
    (
        "Salvar relatório de diagnóstico…",
        "Não foi possível salvar o relatório de diagnóstico. Verifique o destino e tente novamente.",
    ),
    (
        "Salva rapporto diagnostico…",
        "Impossibile salvare il rapporto diagnostico. Controlla la destinazione e riprova.",
    ),
    (
        "Сохранить диагностический отчёт…",
        "Не удалось сохранить диагностический отчёт. Проверьте место сохранения и повторите попытку.",
    ),
];

/// A manual save chooser is the only admission path. Cancellation creates nothing.
/// Saves the support report the user asked for from the diagnostics window, adding the editor's
/// current problem codes and scan-failure counts.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri commands receive owned arguments; the app handle is borrowed for the export"
)]
pub(crate) fn diagnostics_save_report(app: tauri::AppHandle, context: EditorContext) {
    export(&app, Some(context));
}

pub(crate) fn export(app: &tauri::AppHandle, editor: Option<EditorContext>) {
    let Some(permit) = app.state::<Arc<ExportAdmission>>().try_start() else {
        return;
    };
    if let Some(menu) = app.try_state::<crate::status_menu::StatusMenu>() {
        menu.refresh_diagnostics();
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _guard = ExportGuard {
            app: app.clone(),
            permit: Some(permit),
        };
        let locale = crate::native_text::current_locale();
        let text = LABELS[locale.index()];
        let mut chooser = rfd::AsyncFileDialog::new()
            .set_title(text.0)
            .set_file_name("dump.json")
            .add_filter("JSON", &["json"]);
        if let Some(window) = app.get_webview_window("main") {
            chooser = chooser.set_parent(&window);
        }
        let Some(file) = chooser.save_file().await else {
            return;
        };
        let driver = tokio::time::timeout(Duration::from_secs(4), async {
            mixrelay_driver_adapters::DriverClient::session()?
                .snapshot()
                .await
        })
        .await
        .ok()
        .and_then(Result::ok);
        let runtime = tokio::time::timeout(
            Duration::from_secs(4),
            crate::background::dispatch(&app, "control_get_status", Value::Null),
        )
        .await
        .ok()
        .and_then(Result::ok);
        let destination = file.path().to_path_buf();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let report = report(
                driver.as_ref(),
                runtime.as_ref(),
                editor.as_ref(),
                &os_version(),
            );
            save(&destination, &report)
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            rfd::AsyncMessageDialog::new()
                .set_title("MixRelay")
                .set_description(text.1)
                .set_level(rfd::MessageLevel::Error)
                .set_buttons(rfd::MessageButtons::OkCustom(
                    crate::native_text::dialogs::text(
                        locale,
                        crate::native_text::dialogs::Text::Ok,
                    )
                    .into(),
                ))
                .show()
                .await;
        }
    });
}

fn os_version() -> String {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("/usr/bin/sw_vers");
    #[cfg(target_os = "macos")]
    command.arg("-productVersion");
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("/usr/bin/uname");
    #[cfg(target_os = "linux")]
    command.arg("-r");
    command
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .filter(|value| {
            value.len() <= 128
                && value
                    .trim()
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-_+".contains(&byte))
        })
        .map_or_else(|| "unavailable".into(), |value| value.trim().into())
}

fn report(
    driver: Option<&DriverSnapshot>,
    runtime: Option<&Value>,
    editor: Option<&EditorContext>,
    os_version: &str,
) -> Value {
    diagnostics::report(
        driver,
        runtime,
        editor,
        &ReportEnvironment {
            build_version: env!("CARGO_PKG_VERSION"),
            platform: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
            os_version,
            created_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|time| time.as_secs()),
        },
    )
}

/// One private same-directory temporary file, cleaned on failure and atomically consumed on save.
fn save(destination: &Path, report: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(report)?;
    if bytes.len() >= REPORT_LIMIT {
        return Err(std::io::Error::other(
            "diagnostic report exceeds its fixed limit",
        ));
    }
    let parent = destination
        .parent()
        .filter(|parent| parent.is_dir())
        .ok_or_else(|| std::io::Error::other("diagnostic destination has no parent"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(destination).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_explicit_save_replaces_one_file_and_failure_leaves_no_temporary_dump() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        save(&path, &json!({"sequence":1})).unwrap();
        save(&path, &json!({"sequence":2})).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap()["sequence"],
            2
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(save(directory.path(), &json!({})).is_err());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(save(&path, &json!("x".repeat(REPORT_LIMIT))).is_err());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
