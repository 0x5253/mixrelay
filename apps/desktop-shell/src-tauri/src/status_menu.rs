//! The runtime's status menu: device settings, the editor, pause, updates, diagnostics
//! and quit. Audio lifecycle stays with the runtime.
use std::sync::{Mutex, MutexGuard};

use mixrelay_driver_core::DriverSnapshot;
use tauri::{
    AppHandle, Manager, Wry,
    menu::Menu,
    tray::{MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

mod driver_settings;
mod labels;
mod model;
mod playback;
mod render;
mod state;
mod update_checks;

use model::{Environment, ItemId, StatusMenuModel};
use state::MenuState;

pub(crate) struct StatusMenu {
    app: AppHandle,
    state: Mutex<MenuState>,
    tray: Menu<Wry>,
    items: render::FixedItems,
}

impl StatusMenu {
    pub(crate) fn refresh_diagnostics(&self) {
        let _ = self.refresh_menu();
    }

    pub(crate) fn observe_audio_power(
        &self,
        power: Option<mixrelay_control_protocol::EnginePowerStatus>,
    ) {
        if self
            .lock_state()
            .is_some_and(|mut state| state.observe_audio_power(power))
        {
            let _ = self.refresh_menu();
        }
    }

    /// Marks the runtime as stopping; `false` when a stop was already in progress.
    pub(crate) fn set_runtime_stopping(&self, stopping: bool) -> bool {
        let changed = self
            .lock_state()
            .is_some_and(|mut state| state.set_stopping(stopping));
        let _ = self.refresh_menu();
        changed
    }

    /// Relabels native controls and their current values for the selected UI locale.
    pub(crate) fn set_locale(&self, locale: &str) -> tauri::Result<()> {
        if let Some(mut state) = self.lock_state() {
            locale.clone_into(&mut state.locale);
        }
        self.refresh_menu()
    }

    /// Publishes shared Driver mutation readback to native menus.
    pub(crate) fn observe_snapshot(&self, snapshot: &DriverSnapshot) {
        if let Some(mut state) = self.lock_state() {
            state.snapshot = Some(snapshot.clone());
            state.error = None;
        }
        let _ = self.refresh_menu();
    }

    fn lock_state(&self) -> Option<MutexGuard<'_, MenuState>> {
        self.state
            .lock()
            .inspect_err(|_| tracing::error!("status menu state lock is unavailable"))
            .ok()
    }

    /// Schedules a render on the main thread, which owns native menus.
    fn refresh_menu(&self) -> tauri::Result<()> {
        let app = self.app.clone();
        let result = self.app.run_on_main_thread(move || {
            if let Err(error) = app.state::<StatusMenu>().render() {
                tracing::error!(%error, "status menu could not be refreshed");
            }
        });
        if let Err(error) = &result {
            tracing::error!(%error, "status menu refresh could not be scheduled");
        }
        result
    }

    fn render(&self) -> tauri::Result<()> {
        let environment = Environment {
            editor_installed: crate::background::startup::editor_executable().is_ok(),
            diagnostics_exporting: crate::diagnostics::is_exporting(&self.app),
        };
        let Some(mut state) = self.lock_state() else {
            return Ok(());
        };
        state.generation = state.generation.wrapping_add(1);
        let model = StatusMenuModel::build(&state, environment);
        state.actions = model.actions();
        drop(state);
        render::render(&self.app, &self.tray, &self.items, &model)
    }
}

/// Installs the runtime's single status icon.
pub(crate) fn install(app: &tauri::App) -> tauri::Result<()> {
    let tray = Menu::new(app)?;
    app.manage(StatusMenu {
        app: app.handle().clone(),
        state: Mutex::new(MenuState {
            locale: crate::native_text::current_locale().tag().into(),
            ..MenuState::default()
        }),
        tray: tray.clone(),
        items: render::FixedItems::new(app.handle())?,
    });
    app.state::<StatusMenu>().render()?;
    app.on_menu_event(|app, event| handle_menu_event(app, event.id().as_ref()));
    install_tray(app, &tray)?;
    driver_settings::refresh(app.handle());
    update_checks::start(app.handle());
    Ok(())
}

fn install_tray(app: &tauri::App, tray: &Menu<Wry>) -> tauri::Result<()> {
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
                driver_settings::refresh(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn handle_menu_event(app: &AppHandle, id: &str) {
    match ItemId::parse(id) {
        Some(ItemId::Quit) => stop_all(app),
        Some(ItemId::OpenEditor) => open_editor(),
        Some(ItemId::Pause) => playback::toggle(app),
        Some(ItemId::CheckUpdates) => update_checks::check(app, true),
        Some(ItemId::Diagnostics) => crate::diagnostics::export(app, None),
        Some(ItemId::Download) => update_checks::open_download(app),
        Some(ItemId::Status) => {}
        None => {
            let action = app
                .state::<StatusMenu>()
                .lock_state()
                .and_then(|state| state.actions.get(id).cloned());
            if let Some(action) = action {
                driver_settings::apply(app, action);
            }
        }
    }
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
fn stop_all(app: &AppHandle) {
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
pub(crate) async fn shutdown_failed(app: &AppHandle) {
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
