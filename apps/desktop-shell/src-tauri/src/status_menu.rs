use std::{collections::HashMap, sync::Mutex, time::Duration};

use mixrelay_driver_core::DriverSnapshot;
use tauri::{
    Manager,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

mod choices;
mod menu;
mod playback;

use choices::SettingAction;
use menu::labels;

#[derive(Clone, Debug, PartialEq, Eq)]
enum UpdateResult {
    Current,
    NotPublished,
    CheckFailed,
    Download { version: String, url: String },
    DownloadFailed,
}

#[derive(Default)]
struct MenuState {
    locale: String,
    snapshot: Option<DriverSnapshot>,
    busy: bool,
    stopping: bool,
    error: Option<String>,
    update_result: Option<UpdateResult>,
    updates_busy: bool,
    generation: u64,
    actions: HashMap<String, SettingAction>,
    audio_power: Option<mixrelay_control_protocol::EnginePowerStatus>,
    audio_change: Option<bool>,
}

/// The runtime's status menu: free device settings, the editor, pause and quit.
pub(crate) struct StatusMenu {
    app: tauri::AppHandle,
    state: Mutex<MenuState>,
    status: MenuItem<tauri::Wry>,
    tray: Menu<tauri::Wry>,
    updates: MenuItem<tauri::Wry>,
    diagnostics: MenuItem<tauri::Wry>,
    open_editor: MenuItem<tauri::Wry>,
    pause: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

impl StatusMenu {
    pub(crate) fn refresh_diagnostics(&self) {
        let _ = self.refresh_menu();
    }

    pub(crate) fn observe_audio_power(
        &self,
        power: Option<mixrelay_control_protocol::EnginePowerStatus>,
    ) {
        let changed = self.state.lock().is_ok_and(|mut state| {
            if state.audio_power == power {
                return false;
            }
            state.audio_power = power;
            true
        });
        if changed {
            let _ = self.refresh_menu();
        }
    }
    /// Marks the runtime as stopping; `false` when a stop was already in progress.
    pub(crate) fn set_runtime_stopping(&self, stopping: bool) -> bool {
        let changed = self.state.lock().is_ok_and(|mut state| {
            let changed = state.stopping != stopping;
            state.stopping = stopping;
            changed
        });
        let _ = self.refresh_menu();
        changed
    }

    /// Relabels native controls and their current values for the selected UI locale.
    pub(crate) fn set_labels(&self, locale: &str) -> tauri::Result<()> {
        if let Ok(mut state) = self.state.lock() {
            locale.clone_into(&mut state.locale);
        }
        self.refresh_menu()
    }

    /// Publishes shared Driver mutation readback to native menus.
    pub(crate) fn observe_snapshot(&self, snapshot: &DriverSnapshot) {
        match self.state.lock() {
            Ok(mut state) => {
                state.snapshot = Some(snapshot.clone());
                state.error = None;
            }
            Err(error) => tracing::error!(%error, "Driver mutation readback could not be observed"),
        }
        let _ = self.refresh_menu();
    }

    fn refresh_menu(&self) -> tauri::Result<()> {
        let app = self.app.clone();
        let result = self.app.run_on_main_thread(move || {
            let menu = app.state::<StatusMenu>();
            if let Err(error) = menu.render() {
                tracing::error!(%error, "status menu could not be refreshed");
            }
        });
        if let Err(error) = &result {
            tracing::error!(%error, "status menu refresh could not be scheduled");
        }
        result
    }

    fn render(&self) -> tauri::Result<()> {
        let Ok(mut state) = self.state.lock() else {
            tracing::error!("status menu state lock is unavailable");
            return Ok(());
        };
        let text = labels(&state.locale);
        self.updates
            .set_text(menu::update_label(state.updates_busy, &text))?;
        self.updates.set_enabled(!state.updates_busy)?;
        self.diagnostics
            .set_text(crate::diagnostics::label(&state.locale))?;
        self.diagnostics
            .set_enabled(!crate::diagnostics::is_exporting(&self.app))?;
        self.quit.set_text(text.quit)?;
        self.quit.set_enabled(!state.stopping)?;
        self.status.set_text(menu::status_text(&state))?;
        self.open_editor.set_text(open_label(&state.locale))?;
        state.generation = state.generation.wrapping_add(1);
        state.actions.clear();
        for item in self.tray.items()? {
            self.tray.remove(&item)?;
        }
        self.tray.append(&self.status)?;
        self.tray
            .append(&menu::devices_menu(&self.app, &mut state, &text)?)?;
        if crate::background::startup::editor_executable().is_ok() {
            self.tray.append(&self.open_editor)?;
        }
        let power = state.audio_power;
        let pending =
            state.audio_change.is_some() || power.is_some_and(|power| power.transitioning);
        self.pause
            .set_text(playback::label(&state.locale, power, pending))?;
        self.pause
            .set_enabled(power.is_some() && !pending && !state.busy && !state.stopping)?;
        self.tray.append(&self.pause)?;
        self.tray
            .append(&PredefinedMenuItem::separator(&self.app)?)?;
        self.tray.append(&self.updates)?;
        if let Some(result) = &state.update_result {
            let item = MenuItem::with_id(
                &self.app,
                "status:download",
                menu::update_result_label(result, &text),
                matches!(result, UpdateResult::Download { .. }) && !state.updates_busy,
                None::<&str>,
            )?;
            self.tray.append(&item)?;
        }
        self.tray.append(&self.diagnostics)?;
        self.tray.append(&self.quit)
    }
}

/// Installs the runtime's single status icon. Audio lifecycle stays with the runtime.
pub(crate) fn install(app: &tauri::App) -> tauri::Result<()> {
    let (menu, tray) = create_menu(app)?;
    app.manage(menu);
    app.state::<StatusMenu>().render()?;
    app.on_menu_event(|app, event| handle_menu_event(app, &event));
    install_tray(app, &tray)?;
    refresh_snapshot(app.handle());
    start_update_checks(app.handle());
    Ok(())
}

fn create_menu(app: &tauri::App) -> tauri::Result<(StatusMenu, Menu<tauri::Wry>)> {
    let text = labels("en");
    let status = MenuItem::with_id(app, "status:status", "MixRelay", false, None::<&str>)?;
    let updates = MenuItem::with_id(
        app,
        "status:updates",
        text.check_updates,
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "status:quit", text.quit, true, Some("CmdOrCtrl+Q"))?;
    let diagnostics = MenuItem::with_id(
        app,
        "status:diagnostics",
        crate::diagnostics::label("en"),
        true,
        None::<&str>,
    )?;
    let open_editor = MenuItem::with_id(
        app,
        "status:open-editor",
        open_label("en"),
        true,
        None::<&str>,
    )?;
    let pause = MenuItem::with_id(
        app,
        "status:pause",
        playback::label("en", None, false),
        false,
        None::<&str>,
    )?;
    let tray = Menu::new(app)?;
    let menu = StatusMenu {
        app: app.handle().clone(),
        state: Mutex::new(MenuState {
            locale: crate::native_text::current_locale().tag().into(),
            ..MenuState::default()
        }),
        status,
        tray: tray.clone(),
        updates,
        diagnostics,
        open_editor,
        pause,
        quit,
    };
    Ok((menu, tray))
}

fn install_tray(app: &tauri::App, tray: &Menu<tauri::Wry>) -> tauri::Result<()> {
    TrayIconBuilder::with_id("status")
        .icon(tauri::include_image!("tray-icons/64x64.png"))
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("MixRelay")
        .menu(tray)
        .show_menu_on_left_click(true)
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button_state: MouseButtonState::Down,
                    ..
                }
            ) {
                refresh_snapshot(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn start_update_checks(app: &tauri::AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_hours(24));
        loop {
            interval.tick().await;
            check_updates(&handle, false);
        }
    });
}

fn handle_menu_event(app: &tauri::AppHandle, event: &tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        "status:quit" => stop_all(app),
        "status:open-editor" => open_editor(),
        "status:pause" => playback::toggle(app),
        "status:updates" => check_updates(app, true),
        "status:diagnostics" => crate::diagnostics::export(app, None),
        "status:download" => open_download(app),
        id => {
            let action = app
                .state::<StatusMenu>()
                .state
                .lock()
                .ok()
                .and_then(|state| state.actions.get(id).cloned());
            if let Some(action) = action {
                mutate_setting(app, action);
            }
        }
    }
}

fn begin_snapshot(app: &tauri::AppHandle) -> bool {
    let menu = app.state::<StatusMenu>();
    let Ok(mut state) = menu.state.lock() else {
        return false;
    };
    if state.busy || state.audio_change.is_some() || state.stopping {
        return false;
    }
    state.busy = true;
    state.error = None;
    drop(state);
    let _ = menu.refresh_menu();
    true
}

fn refresh_snapshot(app: &tauri::AppHandle) {
    if !begin_snapshot(app) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        playback::refresh(&app).await;
        finish_snapshot(&app, crate::driver_control::driver_status().await);
    });
}

fn finish_snapshot(app: &tauri::AppHandle, result: Result<DriverSnapshot, String>) {
    let menu = app.state::<StatusMenu>();
    if let Ok(mut state) = menu.state.lock() {
        state.busy = false;
        match result {
            Ok(snapshot) => {
                state.snapshot = Some(snapshot);
                state.error = None;
            }
            Err(error) => {
                tracing::error!(%error, "status menu action failed");
                state.error = Some(error);
            }
        }
    }
    let _ = menu.refresh_menu();
}

fn mutate_setting(app: &tauri::AppHandle, action: SettingAction) {
    if !begin_snapshot(app) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = apply_setting(&app, action).await;
        finish_snapshot(&app, result);
    });
}

async fn apply_setting(
    app: &tauri::AppHandle,
    action: SettingAction,
) -> Result<DriverSnapshot, String> {
    let snapshot = crate::driver_control::driver_status().await?;
    let command = match choices::command(&snapshot, &action) {
        Ok(command) => command,
        Err(error) => {
            if let Ok(mut state) = app.state::<StatusMenu>().state.lock() {
                state.snapshot = Some(snapshot);
            }
            return Err(error.to_string());
        }
    };
    let result = crate::driver_control::driver_mutate(app.clone(), command).await;
    if result.is_err()
        && let Ok(snapshot) = crate::driver_control::driver_status().await
        && let Ok(mut state) = app.state::<StatusMenu>().state.lock()
    {
        state.snapshot = Some(snapshot);
    }
    result
}

fn check_updates(app: &tauri::AppHandle, manual: bool) {
    let menu = app.state::<StatusMenu>();
    let Ok(mut state) = menu.state.lock() else {
        return;
    };
    if state.updates_busy {
        return;
    }
    state.updates_busy = true;
    drop(state);
    let _ = menu.refresh_menu();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let update = crate::updates::updates_check_driver().await;
        let menu = app.state::<StatusMenu>();
        if let Ok(mut state) = menu.state.lock() {
            state.update_result = menu::update_result(
                update.state(),
                update.latest_version().zip(
                    update
                        .downloads()
                        .first()
                        .map(crate::updates::Download::url),
                ),
                manual,
            );
            state.updates_busy = false;
        }
        let _ = menu.refresh_menu();
    });
}

fn open_download(app: &tauri::AppHandle) {
    let menu = app.state::<StatusMenu>();
    let Ok(mut state) = menu.state.lock() else {
        return;
    };
    if state.updates_busy {
        return;
    }
    let Some(UpdateResult::Download { url, .. }) = &state.update_result else {
        return;
    };
    let url = url.clone();
    state.updates_busy = true;
    drop(state);
    let _ = menu.refresh_menu();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result =
            crate::updates::updates_open_download(crate::updates::Product::MixRelay, url).await;
        let menu = app.state::<StatusMenu>();
        if let Err(error) = &result {
            tracing::error!(%error, "update download could not be opened");
        }
        if let Ok(mut state) = menu.state.lock() {
            state.updates_busy = false;
            if result.is_err() {
                state.update_result = Some(UpdateResult::DownloadFailed);
            }
        }
        let _ = menu.refresh_menu();
    });
}

const OPEN_LABELS: [&str; 11] = [
    "Open MixRelay",
    "MixRelay 열기",
    "MixRelayを開く",
    "打开 MixRelay",
    "開啟 MixRelay",
    "Ouvrir MixRelay",
    "MixRelay öffnen",
    "Abrir MixRelay",
    "Abrir MixRelay",
    "Apri MixRelay",
    "Открыть MixRelay",
];

fn open_label(locale: &str) -> &'static str {
    OPEN_LABELS[crate::native_text::resolve_locale(locale).index()]
}

pub(crate) fn open_editor() {
    tauri::async_runtime::spawn_blocking(|| {
        let result = crate::background::startup::editor_executable()
            .and_then(|exe| crate::background::spawn_editor(&exe));
        if let Err(error) = result {
            tracing::error!(%error, "could not open MixRelay editor");
        }
    });
}

/// Quit stops all audio first; native/system termination takes the same serialized stop.
fn stop_all(app: &tauri::AppHandle) {
    if !app.state::<StatusMenu>().set_runtime_stopping(true) {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        // EngineLifecycle cancels queued activation and waits for the HAL ACK.
        app.exit(0);
    }
    #[cfg(target_os = "linux")]
    {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            match crate::driver_control::stop_driver().await {
                Ok(_) => app.exit(0),
                Err(error) => {
                    tracing::error!(%error, "MixRelay could not stop all audio");
                    shutdown_failed(&app).await;
                }
            }
        });
    }
}

/// A failed stop keeps the runtime and its Quit available for another attempt.
pub(crate) async fn shutdown_failed(app: &tauri::AppHandle) {
    app.state::<StatusMenu>().set_runtime_stopping(false);
    let locale = crate::native_text::current_locale();
    let (title, body) = crate::native_text::quit_failed(locale);
    rfd::AsyncMessageDialog::new()
        .set_title(title)
        .set_description(body)
        .set_buttons(rfd::MessageButtons::OkCustom(
            crate::native_text::dialogs::text(locale, crate::native_text::dialogs::Text::Ok).into(),
        ))
        .set_level(rfd::MessageLevel::Error)
        .show()
        .await;
}
