//! One acknowledged pause state for the editor and the status menu.

use mixrelay_control_protocol::EnginePowerStatus;
use mixrelay_desktop_core::{
    playback::{PowerAcknowledgement, PowerProgress},
    timing::{AsyncDelay, before},
};
use mixrelay_time::MonotonicClock as _;

use tauri::Manager as _;

use super::StatusMenu;

const LABELS: [[&str; 4]; 11] = [
    [
        "Pause",
        "Resume",
        "Changing audio state…",
        "Could not pause or resume audio",
    ],
    [
        "일시정지",
        "재개",
        "오디오 상태 변경 중…",
        "오디오 일시정지 또는 재개에 실패했습니다",
    ],
    [
        "一時停止",
        "再開",
        "オーディオの状態を変更中…",
        "オーディオの一時停止または再開に失敗しました",
    ],
    ["暂停", "继续", "正在更改音频状态…", "音频暂停或继续失败"],
    ["暫停", "繼續", "正在變更音訊狀態…", "音訊暫停或繼續失敗"],
    [
        "Mettre en pause",
        "Reprendre",
        "Changement de l’état audio…",
        "Impossible de mettre l’audio en pause ou de le reprendre",
    ],
    [
        "Pausieren",
        "Fortsetzen",
        "Audiostatus wird geändert…",
        "Audio konnte nicht pausiert oder fortgesetzt werden",
    ],
    [
        "Pausar",
        "Reanudar",
        "Cambiando el estado del audio…",
        "No se pudo pausar o reanudar el audio",
    ],
    [
        "Pausar",
        "Retomar",
        "Alterando o estado do áudio…",
        "Não foi possível pausar ou retomar o áudio",
    ],
    [
        "Pausa",
        "Riprendi",
        "Cambio dello stato audio…",
        "Impossibile mettere in pausa o riprendere l’audio",
    ],
    [
        "Приостановить",
        "Возобновить",
        "Изменение состояния аудио…",
        "Не удалось приостановить или возобновить аудио",
    ],
];

pub(super) fn label(locale: &str, power: Option<EnginePowerStatus>, pending: bool) -> &'static str {
    let index = if pending {
        2
    } else {
        usize::from(power.is_some_and(|power| !power.enabled))
    };
    LABELS[crate::native_text::resolve_locale(locale).index()][index]
}

async fn status(app: &tauri::AppHandle) -> Result<EnginePowerStatus, String> {
    let response = crate::control_proxy::control_get_status(app.state(), app.clone()).await?;
    if response.status != 200 {
        return Err(response.body.to_string());
    }
    serde_json::from_value(response.body["enginePower"].clone()).map_err(|error| error.to_string())
}

pub(super) async fn refresh(app: &tauri::AppHandle) {
    let result = status(app).await.ok();
    app.state::<StatusMenu>().observe_audio_power(result);
}

pub(super) fn toggle(app: &tauri::AppHandle) {
    let menu = app.state::<StatusMenu>();
    let Ok(mut state) = menu.state.lock() else {
        return;
    };
    let Some(power) = state.audio_power else {
        return;
    };
    if state.audio_change.is_some() || state.busy || power.transitioning || state.stopping {
        return;
    }
    state.audio_change = Some(!power.enabled);
    let locale = state.locale.clone();
    drop(state);
    let _ = menu.refresh_menu();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = change(&app, !power.enabled).await;
        refresh(&app).await;
        if let Ok(mut state) = app.state::<StatusMenu>().state.lock() {
            state.audio_change = None;
        }
        let _ = app.state::<StatusMenu>().refresh_menu();
        if let Err(error) = result {
            tracing::error!(%error, "Driver audio pause/resume failed");
            rfd::AsyncMessageDialog::new()
                .set_title("MixRelay")
                .set_description(LABELS[crate::native_text::resolve_locale(&locale).index()][3])
                .set_level(rfd::MessageLevel::Error)
                .show()
                .await;
        }
    });
}

async fn change(app: &tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let response =
        crate::control_proxy::control_set_engine_power(app.state(), app.clone(), enabled).await?;
    if response.status != 200 {
        return Err(response.body.to_string());
    }
    let timer = crate::async_time::TokioDelay::new(mixrelay_time::StdClock::new());
    let acknowledgement = PowerAcknowledgement::new(enabled, timer.now());
    loop {
        let power = before(&timer, acknowledgement.deadline(), status(app))
            .await
            .ok_or("audio pause acknowledgement timed out")??;
        match acknowledgement.observe(power, timer.now()) {
            PowerProgress::Complete => return Ok(()),
            PowerProgress::Expired => return Err("audio pause acknowledgement timed out".into()),
            PowerProgress::Waiting(deadline) => timer.wait_until(deadline).await,
        }
    }
}
