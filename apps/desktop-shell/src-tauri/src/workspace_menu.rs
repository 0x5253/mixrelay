use std::sync::Mutex;

use tauri::{
    Emitter, Manager,
    menu::{Menu, MenuItem, Submenu},
};

use crate::app_menu::NativeMenuLabels;

pub(crate) struct WorkspaceMenu {
    about: MenuItem<tauri::Wry>,
    file: Submenu<tauri::Wry>,
    new: MenuItem<tauri::Wry>,
    open: MenuItem<tauri::Wry>,
    save: MenuItem<tauri::Wry>,
    save_as: MenuItem<tauri::Wry>,
    view: Submenu<tauri::Wry>,
    graph_list: MenuItem<tauri::Wry>,
    midi_map: MenuItem<tauri::Wry>,
    graph_list_state: GraphListLabel,
    app: tauri::AppHandle,
    #[cfg(target_os = "macos")]
    application_quit: MenuItem<tauri::Wry>,
    #[cfg(target_os = "linux")]
    quit: MenuItem<tauri::Wry>,
}

impl WorkspaceMenu {
    pub(crate) fn set_ready(&self, ready: bool) -> tauri::Result<()> {
        for item in [&self.new, &self.open, &self.save, &self.save_as] {
            item.set_enabled(ready)?;
        }
        Ok(())
    }

    pub(crate) fn set_labels(&self, labels: &NativeMenuLabels) -> tauri::Result<()> {
        self.about.set_text(labels.app_about)?;
        self.file.set_text(labels.workspace_file)?;
        self.new.set_text(labels.workspace_new)?;
        self.open.set_text(labels.workspace_open)?;
        self.save.set_text(labels.workspace_save)?;
        self.save_as.set_text(labels.workspace_save_as)?;
        self.view.set_text(labels.view)?;
        self.midi_map.set_text(labels.midi_map)?;
        self.graph_list_state.set_labels(*labels);
        self.apply_graph_list_label()?;
        #[cfg(target_os = "macos")]
        self.application_quit.set_text(labels.app_quit)?;
        #[cfg(target_os = "linux")]
        self.quit.set_text(labels.quit)?;
        Ok(())
    }

    /// Mirrors the editor's graph list so the command names what it will do next.
    pub(crate) fn set_graph_list_visible(&self, visible: bool) -> tauri::Result<()> {
        self.graph_list_state.set_visible(visible);
        self.apply_graph_list_label()
    }

    /// Locale and visibility updates arrive from different commands. Each schedules one
    /// main-thread write that reads the newest pair at run time, so the last write wins
    /// with both values current instead of an older captured state.
    fn apply_graph_list_label(&self) -> tauri::Result<()> {
        let app = self.app.clone();
        self.app.run_on_main_thread(move || {
            let menu = app.state::<WorkspaceMenu>();
            if let Err(error) = menu.graph_list.set_text(menu.graph_list_state.current()) {
                tracing::error!(%error, "graph list menu label could not be updated");
            }
        })
    }
}

/// The Show/Hide Graph List label inputs, updated together under one lock.
struct GraphListLabel(Mutex<(NativeMenuLabels, bool)>);

impl GraphListLabel {
    const fn new(labels: NativeMenuLabels) -> Self {
        Self(Mutex::new((labels, true)))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (NativeMenuLabels, bool)> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set_labels(&self, labels: NativeMenuLabels) {
        self.lock().0 = labels;
    }

    fn set_visible(&self, visible: bool) {
        self.lock().1 = visible;
    }

    /// The command names the opposite of the current state.
    fn current(&self) -> &'static str {
        let (labels, visible) = *self.lock();
        if visible {
            labels.hide_graph_list
        } else {
            labels.show_graph_list
        }
    }
}

/// The macOS standard Show/Hide Sidebar shortcut; Linux file managers use F9.
const GRAPH_LIST_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "Ctrl+Cmd+S"
} else {
    "F9"
};

/// Applies the editor's graph list visibility to the View command label.
#[tauri::command]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri injects AppHandle by value; a borrowed handle is not a command argument"
)]
pub(crate) fn set_graph_list_visible(visible: bool, app: tauri::AppHandle) -> Result<(), String> {
    app.state::<WorkspaceMenu>()
        .set_graph_list_visible(visible)
        .map_err(|error| error.to_string())
}

pub(crate) fn install(app: &tauri::App) -> tauri::Result<()> {
    let menu = Menu::default(app.handle())?;
    let labels = crate::app_menu::labels("en");
    let about = MenuItem::with_id(app, "app:about", labels.app_about, true, None::<&str>)?;
    #[cfg(target_os = "macos")]
    let application_quit = install_app_menu(app, &menu, &about)?;
    let file = menu
        .items()?
        .into_iter()
        .filter_map(|item| item.as_submenu().cloned())
        .find(|submenu| submenu.text().is_ok_and(|text| text == "File"));
    let file = if let Some(file) = file {
        file
    } else {
        let file = Submenu::new(app, "File", true)?;
        menu.prepend(&file)?;
        file
    };
    let [new, open, save, save_as] = workspace_file_items(app, &labels)?;
    file.insert(&new, 0)?;
    file.insert(&open, 1)?;
    file.insert(&save, 2)?;
    file.insert(&save_as, 3)?;
    let (view, graph_list) = graph_list_item(app, &menu, &labels)?;
    let midi_map = install_midi_map(app, &menu, &view, &labels)?;
    #[cfg(target_os = "linux")]
    let quit = {
        let quit = MenuItem::with_id(app, "app:quit", labels.quit, true, Some("Ctrl+Q"))?;
        file.append(&tauri::menu::PredefinedMenuItem::separator(app)?)?;
        file.append(&about)?;
        file.append(&quit)?;
        quit
    };
    app.set_menu(menu)?;
    app.manage(WorkspaceMenu {
        about,
        file,
        new,
        open,
        save,
        save_as,
        view,
        graph_list,
        midi_map,
        graph_list_state: GraphListLabel::new(labels),
        app: app.handle().clone(),
        #[cfg(target_os = "macos")]
        application_quit,
        #[cfg(target_os = "linux")]
        quit,
    });
    app.state::<WorkspaceMenu>().set_ready(false)?;
    app.on_menu_event(|app, event| handle_menu_event(app, &event));
    Ok(())
}

fn handle_menu_event(app: &tauri::AppHandle, event: &tauri::menu::MenuEvent) {
    if event.id().as_ref() == "app:about" {
        let _ = crate::app_menu::show_main_window(app);
        let _ = app.emit("app-about", ());
        return;
    }
    if event.id().as_ref() == "view:midi-map" {
        let _ = app.emit("midi-map-toggle", ());
        return;
    }
    if event.id().as_ref() == "window:minimise" {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.minimize();
        }
        return;
    }
    if event.id().as_ref() == "view:graph-list" {
        // A closed editor only hides its window; bring it back so the change is seen.
        let _ = crate::app_menu::show_main_window(app);
        let _ = app.emit("graph-list-toggle", ());
        return;
    }
    if event.id().as_ref() == "app:quit" {
        app.exit(0);
        return;
    }
    if let Some(action) = event.id().as_ref().strip_prefix("workspace:") {
        let _ = app.emit("workspace-file", action);
    }
}

/// Native accelerators consume Cmd+M before the webview can handle it.
fn install_midi_map(
    app: &tauri::App,
    menu: &Menu<tauri::Wry>,
    view: &Submenu<tauri::Wry>,
    labels: &NativeMenuLabels,
) -> tauri::Result<MenuItem<tauri::Wry>> {
    #[cfg(target_os = "macos")]
    if let Some(window) = menu
        .get(tauri::menu::WINDOW_SUBMENU_ID)
        .and_then(|item| item.as_submenu().cloned())
        && let Some(item) = window.remove_at(0)?
    {
        let text = item
            .as_predefined_menuitem()
            .map(tauri::menu::PredefinedMenuItem::text)
            .transpose()?
            .unwrap_or_else(|| "Minimise".into());
        window.insert(
            &MenuItem::with_id(app, "window:minimise", text, true, None::<&str>)?,
            0,
        )?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = menu;
    let item = MenuItem::with_id(
        app,
        "view:midi-map",
        labels.midi_map,
        true,
        Some("CmdOrCtrl+M"),
    )?;
    view.append(&item)?;
    Ok(item)
}

/// View → Show/Hide Graph List, at the top of the platform View menu.
fn graph_list_item(
    app: &tauri::App,
    menu: &Menu<tauri::Wry>,
    labels: &NativeMenuLabels,
) -> tauri::Result<(Submenu<tauri::Wry>, MenuItem<tauri::Wry>)> {
    let view = menu
        .items()?
        .into_iter()
        .filter_map(|item| item.as_submenu().cloned())
        .find(|submenu| submenu.text().is_ok_and(|text| text == "View"));
    let view = if let Some(view) = view {
        view
    } else {
        let view = Submenu::new(app, "View", true)?;
        menu.append(&view)?;
        view
    };
    let graph_list = MenuItem::with_id(
        app,
        "view:graph-list",
        labels.hide_graph_list,
        true,
        Some(GRAPH_LIST_SHORTCUT),
    )?;
    view.insert(&graph_list, 0)?;
    if view.items()?.len() > 1 {
        view.insert(&tauri::menu::PredefinedMenuItem::separator(app)?, 1)?;
    }
    Ok((view, graph_list))
}

fn workspace_file_items(
    app: &tauri::App,
    labels: &NativeMenuLabels,
) -> tauri::Result<[MenuItem<tauri::Wry>; 4]> {
    let item = |id, label, shortcut| MenuItem::with_id(app, id, label, true, Some(shortcut));
    Ok([
        item("workspace:new", labels.workspace_new, "CmdOrCtrl+N")?,
        item("workspace:open", labels.workspace_open, "CmdOrCtrl+O")?,
        item("workspace:save", labels.workspace_save, "CmdOrCtrl+S")?,
        item(
            "workspace:save_as",
            labels.workspace_save_as,
            "CmdOrCtrl+Shift+S",
        )?,
    ])
}

/// Native predefined Quit calls `NSApp` terminate directly, bypassing `ExitRequested`.
/// Route the menu and shortcut through the preventable exit event so the engine can finalize recordings.
#[cfg(target_os = "macos")]
fn install_app_menu(
    app: &tauri::App,
    menu: &Menu<tauri::Wry>,
    about: &MenuItem<tauri::Wry>,
) -> tauri::Result<MenuItem<tauri::Wry>> {
    use tauri::menu::PredefinedMenuItem;
    let quit = MenuItem::with_id(
        app,
        "app:quit",
        crate::app_menu::labels("en").app_quit,
        true,
        Some("Cmd+Q"),
    )?;
    let application = Submenu::with_items(
        app,
        "MixRelay",
        true,
        &[
            about,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    // Menu::default on macOS starts with its application submenu.
    menu.remove_at(0)?;
    menu.prepend(&application)?;
    Ok(quit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_list_label_reflects_the_newest_locale_and_visibility_in_any_order() {
        let english = crate::app_menu::labels("en");
        let korean = crate::app_menu::labels("ko-KR");
        let state = GraphListLabel::new(english);
        assert_eq!(state.current(), english.hide_graph_list);
        state.set_visible(false);
        state.set_labels(korean);
        assert_eq!(state.current(), korean.show_graph_list);
        state.set_labels(english);
        state.set_visible(true);
        assert_eq!(state.current(), english.hide_graph_list);
    }
}
