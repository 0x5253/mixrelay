//! The editor menu's own items, their shortcuts and labels, and the commands they send.
use super::labels::Labels;

/// An item this app adds to the platform menu bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Item {
    About,
    New,
    Open,
    Save,
    SaveAs,
    GraphList,
    MidiMap,
    Quit,
}

/// The macOS standard Show/Hide Sidebar shortcut; Linux file managers use F9.
const GRAPH_LIST_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "Ctrl+Cmd+S"
} else {
    "F9"
};

impl Item {
    pub(super) const ALL: [Self; 8] = [
        Self::About,
        Self::New,
        Self::Open,
        Self::Save,
        Self::SaveAs,
        Self::GraphList,
        Self::MidiMap,
        Self::Quit,
    ];

    /// File menu commands, in menu order. They stay disabled until the editor is ready.
    pub(super) const FILE: [Self; 4] = [Self::New, Self::Open, Self::Save, Self::SaveAs];

    pub(super) const fn id(self) -> &'static str {
        match self {
            Self::About => "app:about",
            Self::New => "workspace:new",
            Self::Open => "workspace:open",
            Self::Save => "workspace:save",
            Self::SaveAs => "workspace:save_as",
            Self::GraphList => "view:graph-list",
            Self::MidiMap => "view:midi-map",
            Self::Quit => "app:quit",
        }
    }

    pub(super) const fn accelerator(self) -> Option<&'static str> {
        match self {
            Self::About => None,
            Self::New => Some("CmdOrCtrl+N"),
            Self::Open => Some("CmdOrCtrl+O"),
            Self::Save => Some("CmdOrCtrl+S"),
            Self::SaveAs => Some("CmdOrCtrl+Shift+S"),
            Self::GraphList => Some(GRAPH_LIST_SHORTCUT),
            // Native accelerators consume Cmd+M before the webview can handle it.
            Self::MidiMap => Some("CmdOrCtrl+M"),
            Self::Quit if cfg!(target_os = "macos") => Some("Cmd+Q"),
            Self::Quit => Some("Ctrl+Q"),
        }
    }

    /// The Graph List command names the opposite of the list's current visibility.
    pub(super) const fn label(self, labels: &Labels, graph_list_visible: bool) -> &'static str {
        match self {
            Self::About => labels.app_about,
            Self::New => labels.workspace_new,
            Self::Open => labels.workspace_open,
            Self::Save => labels.workspace_save,
            Self::SaveAs => labels.workspace_save_as,
            Self::GraphList if graph_list_visible => labels.hide_graph_list,
            Self::GraphList => labels.show_graph_list,
            Self::MidiMap => labels.midi_map,
            Self::Quit if cfg!(target_os = "macos") => labels.app_quit,
            Self::Quit => labels.quit,
        }
    }
}

/// What a menu event asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Command<'a> {
    ShowAbout,
    ToggleMidiMap,
    Minimise,
    ToggleGraphList,
    Quit,
    /// A File command the editor runs, named by its action (`new`, `save_as`, …).
    WorkspaceFile(&'a str),
}

/// The macOS Window menu's Minimise, replaced so Cmd+M can map MIDI instead.
pub(super) const MINIMISE_ID: &str = "window:minimise";

impl<'a> Command<'a> {
    pub(super) fn parse(id: &'a str) -> Option<Self> {
        Some(match id {
            "app:about" => Self::ShowAbout,
            "view:midi-map" => Self::ToggleMidiMap,
            MINIMISE_ID => Self::Minimise,
            "view:graph-list" => Self::ToggleGraphList,
            "app:quit" => Self::Quit,
            _ => Self::WorkspaceFile(id.strip_prefix("workspace:")?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_menu::labels::labels;

    #[test]
    fn every_item_sends_its_own_command() {
        let expected = [
            Command::ShowAbout,
            Command::WorkspaceFile("new"),
            Command::WorkspaceFile("open"),
            Command::WorkspaceFile("save"),
            Command::WorkspaceFile("save_as"),
            Command::ToggleGraphList,
            Command::ToggleMidiMap,
            Command::Quit,
        ];
        for (item, command) in Item::ALL.into_iter().zip(expected) {
            assert_eq!(Command::parse(item.id()), Some(command), "{item:?}");
        }
        assert_eq!(Command::parse(MINIMISE_ID), Some(Command::Minimise));
        assert_eq!(Command::parse("status:quit"), None);
        assert_eq!(Command::parse("view:unknown"), None);
    }

    #[test]
    fn file_commands_use_the_standard_shortcuts_and_quit_follows_the_platform() {
        let shortcuts: Vec<_> = Item::FILE
            .into_iter()
            .map(|item| item.accelerator().unwrap())
            .collect();
        assert_eq!(
            shortcuts,
            [
                "CmdOrCtrl+N",
                "CmdOrCtrl+O",
                "CmdOrCtrl+S",
                "CmdOrCtrl+Shift+S"
            ]
        );
        assert_eq!(Item::About.accelerator(), None);
        assert_eq!(Item::MidiMap.accelerator(), Some("CmdOrCtrl+M"));
        if cfg!(target_os = "macos") {
            assert_eq!(Item::GraphList.accelerator(), Some("Ctrl+Cmd+S"));
            assert_eq!(Item::Quit.accelerator(), Some("Cmd+Q"));
        } else {
            assert_eq!(Item::GraphList.accelerator(), Some("F9"));
            assert_eq!(Item::Quit.accelerator(), Some("Ctrl+Q"));
        }
    }

    #[test]
    fn items_are_labelled_in_the_selected_language() {
        let korean = labels("ko-KR");
        let names: Vec<_> = Item::ALL
            .into_iter()
            .map(|item| item.label(&korean, true))
            .collect();
        let quit = if cfg!(target_os = "macos") {
            "MixRelay 종료"
        } else {
            "종료"
        };
        assert_eq!(
            names,
            [
                "MixRelay 정보",
                "새 작업 공간",
                "열기…",
                "저장",
                "다른 이름으로 저장…",
                "그래프 목록 가리기",
                "MIDI 매핑 모드",
                quit,
            ]
        );
        assert_eq!(Item::GraphList.label(&korean, false), "그래프 목록 보기");
    }
}
