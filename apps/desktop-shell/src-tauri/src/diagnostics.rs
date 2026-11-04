//! Explicit, bounded support export. No recorder, timer, retained dump, or upload.
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use mixrelay_desktop_core::diagnostics::{self, ExportPermit, REPORT_LIMIT, ReportEnvironment};
pub(crate) use mixrelay_desktop_core::diagnostics::{EditorContext, ExportAdmission};
use mixrelay_driver_core::DriverSnapshot;
use serde_json::Value;
use tauri::{AppHandle, Manager as _};

use crate::native_text::{Locale, current_locale, dialogs, resolve_locale};

/// A stalled Driver or runtime cannot hold up a save the user already confirmed.
const SOURCE_TIMEOUT: Duration = Duration::from_secs(4);

struct Text {
    save_report: &'static str,
    save_failed: &'static str,
}

const TEXT: [Text; 11] = [
    Text {
        save_report: "Save Diagnostic Report…",
        save_failed: "Could not save the diagnostic report. Check the destination and try again.",
    },
    Text {
        save_report: "진단 리포트 저장…",
        save_failed: "진단 리포트를 저장하지 못했습니다. 저장 위치를 확인하고 다시 시도하십시오.",
    },
    Text {
        save_report: "診断レポートを保存…",
        save_failed: "診断レポートを保存できませんでした。保存先を確認して再試行してください。",
    },
    Text {
        save_report: "保存诊断报告…",
        save_failed: "无法保存诊断报告。请检查保存位置并重试。",
    },
    Text {
        save_report: "儲存診斷報告…",
        save_failed: "無法儲存診斷報告。請檢查儲存位置並重試。",
    },
    Text {
        save_report: "Enregistrer un rapport de diagnostic…",
        save_failed: "Impossible d’enregistrer le rapport de diagnostic. Vérifiez la destination et réessayez.",
    },
    Text {
        save_report: "Diagnosebericht speichern…",
        save_failed: "Der Diagnosebericht konnte nicht gespeichert werden. Prüfen Sie den Speicherort und versuchen Sie es erneut.",
    },
    Text {
        save_report: "Guardar informe de diagnóstico…",
        save_failed: "No se pudo guardar el informe de diagnóstico. Compruebe el destino e inténtelo de nuevo.",
    },
    Text {
        save_report: "Salvar relatório de diagnóstico…",
        save_failed: "Não foi possível salvar o relatório de diagnóstico. Verifique o destino e tente novamente.",
    },
    Text {
        save_report: "Salva rapporto diagnostico…",
        save_failed: "Impossibile salvare il rapporto diagnostico. Controlla la destinazione e riprova.",
    },
    Text {
        save_report: "Сохранить диагностический отчёт…",
        save_failed: "Не удалось сохранить диагностический отчёт. Проверьте место сохранения и повторите попытку.",
    },
];

/// The menu item and save chooser title.
pub(crate) fn label(locale: &str) -> &'static str {
    TEXT[resolve_locale(locale).index()].save_report
}

pub(crate) fn is_exporting(app: &AppHandle) -> bool {
    app.state::<Arc<ExportAdmission>>().exporting()
}

/// Ends the export and re-enables the status menu item, however the export finishes.
struct ExportGuard {
    app: AppHandle,
    permit: Option<ExportPermit>,
}

impl Drop for ExportGuard {
    fn drop(&mut self) {
        self.permit.take();
        refresh_status_menu(&self.app);
    }
}

fn refresh_status_menu(app: &AppHandle) {
    if let Some(menu) = app.try_state::<crate::status_menu::StatusMenu>() {
        menu.refresh_diagnostics();
    }
}

/// Saves the support report the user asked for from the diagnostics window, adding the editor's
/// current problem codes and scan-failure counts.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri commands receive owned arguments; the app handle is borrowed for the export"
)]
pub(crate) fn diagnostics_save_report(app: AppHandle, context: EditorContext) {
    export(&app, Some(context));
}

/// A manual save chooser is the only admission path. Cancellation creates nothing.
pub(crate) fn export(app: &AppHandle, editor: Option<EditorContext>) {
    let Some(permit) = app.state::<Arc<ExportAdmission>>().try_start() else {
        return;
    };
    refresh_status_menu(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _guard = ExportGuard {
            app: app.clone(),
            permit: Some(permit),
        };
        let locale = current_locale();
        let Some(destination) = choose_destination(&app, locale).await else {
            return;
        };
        let (driver, runtime) = collect_sources(&app).await;
        let saved = tauri::async_runtime::spawn_blocking(move || {
            let report = report(
                driver.as_ref(),
                runtime.as_ref(),
                editor.as_ref(),
                &os_version(),
            );
            save(&destination, &report)
        })
        .await;
        if !matches!(saved, Ok(Ok(()))) {
            show_save_failure(locale).await;
        }
    });
}

async fn choose_destination(app: &AppHandle, locale: Locale) -> Option<PathBuf> {
    let mut chooser = rfd::AsyncFileDialog::new()
        .set_title(TEXT[locale.index()].save_report)
        .set_file_name("dump.json")
        .add_filter("JSON", &["json"]);
    if let Some(window) = app.get_webview_window("main") {
        chooser = chooser.set_parent(&window);
    }
    Some(chooser.save_file().await?.path().to_path_buf())
}

/// Reads the Driver snapshot and runtime status; either may be missing from the report.
async fn collect_sources(app: &AppHandle) -> (Option<DriverSnapshot>, Option<Value>) {
    let driver = tokio::time::timeout(SOURCE_TIMEOUT, async {
        mixrelay_driver_adapters::DriverClient::session()?
            .snapshot()
            .await
    });
    let runtime = tokio::time::timeout(
        SOURCE_TIMEOUT,
        crate::background::dispatch(app, "control_get_status", Value::Null),
    );
    let driver = driver.await.ok().and_then(Result::ok);
    let runtime = runtime.await.ok().and_then(Result::ok);
    (driver, runtime)
}

async fn show_save_failure(locale: Locale) {
    rfd::AsyncMessageDialog::new()
        .set_title("MixRelay")
        .set_description(TEXT[locale.index()].save_failed)
        .set_level(rfd::MessageLevel::Error)
        .set_buttons(rfd::MessageButtons::OkCustom(
            dialogs::text(locale, dialogs::Text::Ok).into(),
        ))
        .show()
        .await;
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
        .as_deref()
        .and_then(version_text)
        .map_or_else(|| "unavailable".into(), str::to_owned)
}

/// Accepts only a short version string, so the report cannot carry other command output.
fn version_text(output: &str) -> Option<&str> {
    let version = output.trim();
    (output.len() <= 128
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_+".contains(&byte)))
    .then_some(version)
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
    use serde_json::json;

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

    #[test]
    fn the_report_names_this_build_and_platform_without_a_driver_or_runtime() {
        let report = report(None, None, None, "15.1");
        assert_eq!(report["buildVersion"], env!("CARGO_PKG_VERSION"));
        assert_eq!(report["platform"], std::env::consts::OS);
        assert_eq!(report["architecture"], std::env::consts::ARCH);
        assert_eq!(report["osVersion"], "15.1");
        assert_eq!(report["driver"]["available"], false);
        assert!(report["createdUnixSeconds"].as_u64().is_some());
    }

    #[test]
    fn only_a_short_plain_os_version_reaches_the_report() {
        assert_eq!(version_text("15.1.1\n"), Some("15.1.1"));
        assert_eq!(
            version_text("6.8.0-45-generic+rt\n"),
            Some("6.8.0-45-generic+rt")
        );
        assert_eq!(version_text("host.local 15.1"), None);
        assert_eq!(version_text("15.1; rm -rf"), None);
        assert_eq!(version_text(&"1".repeat(129)), None);
    }

    #[test]
    fn the_export_is_labelled_and_reports_failure_in_each_language() {
        assert_eq!(label("en"), "Save Diagnostic Report…");
        assert_eq!(label("ko-KR"), "진단 리포트 저장…");
        assert_eq!(label("zh-TW"), "儲存診斷報告…");
        for (index, text) in TEXT.iter().enumerate().skip(1) {
            assert_ne!(text.save_report, TEXT[0].save_report, "{index}");
            assert_ne!(text.save_failed, TEXT[0].save_failed, "{index}");
            assert!(!text.save_failed.is_empty());
        }
    }
}
