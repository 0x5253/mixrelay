//! One acknowledged pause state for the editor and the status menu.

use mixrelay_control_protocol::EnginePowerStatus;
use mixrelay_desktop_core::{
    playback::{PowerAcknowledgement, PowerProgress},
    timing::{AsyncDelay, before},
};
use mixrelay_time::MonotonicClock as _;
use tauri::{AppHandle, Manager as _};

use super::{StatusMenu, labels::labels};

async fn status(app: &AppHandle) -> Result<EnginePowerStatus, String> {
    let response = crate::control_proxy::control_get_status(app.state(), app.clone()).await?;
    if response.status != 200 {
        return Err(response.body.to_string());
    }
    serde_json::from_value(response.body["enginePower"].clone()).map_err(|error| error.to_string())
}

pub(super) async fn refresh(app: &AppHandle) {
    let result = status(app).await.ok();
    app.state::<StatusMenu>().observe_audio_power(result);
}

/// Pauses running audio or resumes paused audio, reporting a failure in a dialog.
pub(super) fn toggle(app: &AppHandle) {
    let menu = app.state::<StatusMenu>();
    let Some((enabled, locale)) = menu.lock_state().and_then(|mut state| {
        let enabled = state.begin_audio_change()?;
        Some((enabled, state.locale.clone()))
    }) else {
        return;
    };
    let _ = menu.refresh_menu();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = change(&app, enabled).await;
        refresh(&app).await;
        let menu = app.state::<StatusMenu>();
        if let Some(mut state) = menu.lock_state() {
            state.audio_change = None;
        }
        let _ = menu.refresh_menu();
        if let Err(error) = result {
            tracing::error!(%error, "Driver audio pause/resume failed");
            rfd::AsyncMessageDialog::new()
                .set_title("MixRelay")
                .set_description(labels(&locale).pause_failed)
                .set_level(rfd::MessageLevel::Error)
                .show()
                .await;
        }
    });
}

/// Requests the power state and waits until the engine acknowledges it.
async fn change(app: &AppHandle, enabled: bool) -> Result<(), String> {
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
