//! The editor's menu bar: workspace files, View commands, About and Quit.
use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard, PoisonError},
};

use tauri::{
    AppHandle, Emitter, Manager, Wry,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
};

mod items;
mod labels;

use items::{Command, Item};
use labels::{Labels, labels};

pub(crate) struct WorkspaceMenu {
    app: AppHandle,
    file: Submenu<Wry>,
    view: Submenu<Wry>,
    items: HashMap<Item, MenuItem<Wry>>,
    graph_list: GraphListLabel,
}

impl WorkspaceMenu {
    /// Enables the File commands once the editor can handle them.
    pub(crate) fn set_ready(&self, ready: bool) -> tauri::Result<()> {
        for item in Item::FILE {
            self.items[&item].set_enabled(ready)?;
        }
        Ok(())
    }

    /// Relabels every item for the UI locale.
    pub(crate) fn set_locale(&self, locale: &str) -> tauri::Result<()> {
        let labels = labels(locale);
        self.file.set_text(labels.workspace_file)?;
        self.view.set_text(labels.view)?;
        for item in Item::ALL {
            if item != Item::GraphList {
                self.items[&item].set_text(item.label(&labels, true))?;
            }
        }
        self.graph_list.set_labels(labels);
        self.apply_graph_list_label()
    }

    /// Mirrors the editor's graph list so the command names what it will do next.
    pub(crate) fn set_graph_list_visible(&self, visible: bool) -> tauri::Result<()> {
        self.graph_list.set_visible(visible);
        self.apply_graph_list_label()
    }

    /// Locale and visibility updates arrive from different commands. Each schedules one
    /// main-thread write that reads the newest pair at run time, so the last write wins
    /// with both values current instead of an older captured state.
    fn apply_graph_list_label(&self) -> tauri::Result<()> {
        let app = self.app.clone();
        self.app.run_on_main_thread(move || {
            let menu = app.state::<WorkspaceMenu>();
            let label = menu.graph_list.current();
            if let Err(error) = menu.items[&Item::GraphList].set_text(label) {
                tracing::error!(%error, "graph list menu label could not be updated");
            }
        })
    }
}

/// The Show/Hide Graph List label inputs, updated together under one lock.
struct GraphListLabel(Mutex<(Labels, bool)>);

impl GraphListLabel {
    const fn new(labels: Labels) -> Self {
        Self(Mutex::new((labels, true)))
    }

    fn lock(&self) -> MutexGuard<'_, (Labels, bool)> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set_labels(&self, labels: Labels) {
        self.lock().0 = labels;
    }

    fn set_visible(&self, visible: bool) {
        self.lock().1 = visible;
    }

    fn current(&self) -> &'static str {
        let (labels, visible) = *self.lock();
        Item::GraphList.label(&labels, visible)
    }
}

/// Applies the editor's graph list visibility to the View command label.
#[tauri::command]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri injects AppHandle by value; a borrowed handle is not a command argument"
)]
pub(crate) fn set_graph_list_visible(visible: bool, app: AppHandle) -> Result<(), String> {
    app.state::<WorkspaceMenu>()
        .set_graph_list_visible(visible)
        .map_err(|error| error.to_string())
}

/// Adds the editor's items to the platform default menu bar, File commands disabled.
pub(crate) fn install(app: &tauri::App) -> tauri::Result<()> {
    let labels = labels("en");
    let items: HashMap<_, _> = Item::ALL
        .into_iter()
        .map(|item| {
            let label = item.label(&labels, true);
            MenuItem::with_id(app, item.id(), label, true, item.accelerator()).map(|it| (item, it))
        })
        .collect::<tauri::Result<_>>()?;
    let menu = Menu::default(app.handle())?;
    #[cfg(target_os = "macos")]
    replace_application_menu(app, &menu, &items)?;
    let file = top_level(app, &menu, "File", Placement::First)?;
    for (index, item) in Item::FILE.into_iter().enumerate() {
        file.insert(&items[&item], index)?;
    }
    let view = top_level(app, &menu, "View", Placement::Last)?;
    view.insert(&items[&Item::GraphList], 0)?;
    if view.items()?.len() > 1 {
        view.insert(&PredefinedMenuItem::separator(app)?, 1)?;
    }
    #[cfg(target_os = "macos")]
    replace_minimise(app, &menu)?;
    view.append(&items[&Item::MidiMap])?;
    #[cfg(target_os = "linux")]
    {
        file.append(&PredefinedMenuItem::separator(app)?)?;
        file.append(&items[&Item::About])?;
        file.append(&items[&Item::Quit])?;
    }
    app.set_menu(menu)?;
    app.manage(WorkspaceMenu {
        app: app.handle().clone(),
        file,
        view,
        items,
        graph_list: GraphListLabel::new(labels),
    });
    app.state::<WorkspaceMenu>().set_ready(false)?;
    app.on_menu_event(|app, event| {
        if let Some(command) = Command::parse(event.id().as_ref()) {
            run(app, command);
        }
    });
    Ok(())
}

fn run(app: &AppHandle, command: Command<'_>) {
    match command {
        Command::ShowAbout => {
            let _ = crate::app_menu::show_main_window(app);
            let _ = app.emit("app-about", ());
        }
        Command::ToggleMidiMap => {
            let _ = app.emit("midi-map-toggle", ());
        }
        Command::Minimise => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.minimize();
            }
        }
        Command::ToggleGraphList => {
            // A closed editor only hides its window; bring it back so the change is seen.
            let _ = crate::app_menu::show_main_window(app);
            let _ = app.emit("graph-list-toggle", ());
        }
        Command::Quit => app.exit(0),
        Command::WorkspaceFile(action) => {
            let _ = app.emit("workspace-file", action);
        }
    }
}

#[derive(Clone, Copy)]
enum Placement {
    First,
    Last,
}

/// The platform default submenu titled `title`, or a new one placed as given.
fn top_level(
    app: &tauri::App,
    menu: &Menu<Wry>,
    title: &str,
    placement: Placement,
) -> tauri::Result<Submenu<Wry>> {
    let existing = menu
        .items()?
        .into_iter()
        .filter_map(|item| item.as_submenu().cloned())
        .find(|submenu| submenu.text().is_ok_and(|text| text == title));
    if let Some(submenu) = existing {
        return Ok(submenu);
    }
    let submenu = Submenu::new(app, title, true)?;
    match placement {
        Placement::First => menu.prepend(&submenu)?,
        Placement::Last => menu.append(&submenu)?,
    }
    Ok(submenu)
}

/// Native predefined Quit calls `NSApp` terminate directly, bypassing `ExitRequested`.
/// Route the menu and shortcut through the preventable exit event so the engine can
/// finalize recordings.
#[cfg(target_os = "macos")]
fn replace_application_menu(
    app: &tauri::App,
    menu: &Menu<Wry>,
    items: &HashMap<Item, MenuItem<Wry>>,
) -> tauri::Result<()> {
    let application = Submenu::with_items(
        app,
        "MixRelay",
        true,
        &[
            &items[&Item::About],
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &items[&Item::Quit],
        ],
    )?;
    // Menu::default on macOS starts with its application submenu.
    menu.remove_at(0)?;
    menu.prepend(&application)
}

/// Replaces the predefined Minimise, whose Cmd+M would consume the MIDI Map shortcut,
/// with a plain item that keeps the platform's label.
#[cfg(target_os = "macos")]
fn replace_minimise(app: &tauri::App, menu: &Menu<Wry>) -> tauri::Result<()> {
    let Some(window) = menu
        .get(tauri::menu::WINDOW_SUBMENU_ID)
        .and_then(|item| item.as_submenu().cloned())
    else {
        return Ok(());
    };
    let Some(item) = window.remove_at(0)? else {
        return Ok(());
    };
    let text = item
        .as_predefined_menuitem()
        .map(PredefinedMenuItem::text)
        .transpose()?
        .unwrap_or_else(|| "Minimise".into());
    window.insert(
        &MenuItem::with_id(app, items::MINIMISE_ID, text, true, None::<&str>)?,
        0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_list_label_reflects_the_newest_locale_and_visibility_in_any_order() {
        let english = labels("en");
        let korean = labels("ko-KR");
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
