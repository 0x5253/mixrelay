//! Editor menu text for the supported UI languages.

#[derive(Clone, Copy, Debug)]
pub(super) struct Labels {
    pub(super) app_about: &'static str,
    /// Quit in the macOS application menu.
    pub(super) app_quit: &'static str,
    /// Quit in the Linux File menu.
    pub(super) quit: &'static str,
    pub(super) workspace_file: &'static str,
    pub(super) workspace_new: &'static str,
    pub(super) workspace_open: &'static str,
    pub(super) workspace_save: &'static str,
    pub(super) workspace_save_as: &'static str,
    pub(super) view: &'static str,
    pub(super) midi_map: &'static str,
    pub(super) show_graph_list: &'static str,
    pub(super) hide_graph_list: &'static str,
}

const LABELS: [Labels; 11] = [
    Labels {
        app_about: "About MixRelay",
        app_quit: "Quit MixRelay",
        quit: "Quit",
        workspace_file: "File",
        workspace_new: "New Workspace",
        workspace_open: "Open…",
        workspace_save: "Save",
        workspace_save_as: "Save As…",
        view: "View",
        midi_map: "MIDI Map Mode",
        show_graph_list: "Show Graph List",
        hide_graph_list: "Hide Graph List",
    },
    Labels {
        app_about: "MixRelay 정보",
        app_quit: "MixRelay 종료",
        quit: "종료",
        workspace_file: "파일",
        workspace_new: "새 작업 공간",
        workspace_open: "열기…",
        workspace_save: "저장",
        workspace_save_as: "다른 이름으로 저장…",
        view: "보기",
        midi_map: "MIDI 매핑 모드",
        show_graph_list: "그래프 목록 보기",
        hide_graph_list: "그래프 목록 가리기",
    },
    Labels {
        app_about: "MixRelayについて",
        app_quit: "MixRelayを終了",
        quit: "終了",
        workspace_file: "ファイル",
        workspace_new: "新規ワークスペース",
        workspace_open: "開く…",
        workspace_save: "保存",
        workspace_save_as: "名前を付けて保存…",
        view: "表示",
        midi_map: "MIDIマップモード",
        show_graph_list: "グラフ一覧を表示",
        hide_graph_list: "グラフ一覧を隠す",
    },
    Labels {
        app_about: "关于 MixRelay",
        app_quit: "退出 MixRelay",
        quit: "退出",
        workspace_file: "文件",
        workspace_new: "新建工作区",
        workspace_open: "打开…",
        workspace_save: "保存",
        workspace_save_as: "另存为…",
        view: "显示",
        midi_map: "MIDI映射模式",
        show_graph_list: "显示音频图列表",
        hide_graph_list: "隐藏音频图列表",
    },
    Labels {
        app_about: "關於 MixRelay",
        app_quit: "結束 MixRelay",
        quit: "結束",
        workspace_file: "檔案",
        workspace_new: "新增工作區",
        workspace_open: "開啟…",
        workspace_save: "儲存",
        workspace_save_as: "另存新檔…",
        view: "顯示",
        midi_map: "MIDI對應模式",
        show_graph_list: "顯示音訊圖列表",
        hide_graph_list: "隱藏音訊圖列表",
    },
    Labels {
        app_about: "À propos de MixRelay",
        app_quit: "Quitter MixRelay",
        quit: "Quitter",
        workspace_file: "Fichier",
        workspace_new: "Nouvel espace de travail",
        workspace_open: "Ouvrir…",
        workspace_save: "Enregistrer",
        workspace_save_as: "Enregistrer sous…",
        view: "Présentation",
        midi_map: "Mode de mapping MIDI",
        show_graph_list: "Afficher la liste des graphes",
        hide_graph_list: "Masquer la liste des graphes",
    },
    Labels {
        app_about: "Über MixRelay",
        app_quit: "MixRelay beenden",
        quit: "Beenden",
        workspace_file: "Datei",
        workspace_new: "Neuer Arbeitsbereich",
        workspace_open: "Öffnen…",
        workspace_save: "Speichern",
        workspace_save_as: "Speichern unter…",
        view: "Darstellung",
        midi_map: "MIDI-Zuweisungsmodus",
        show_graph_list: "Graphenliste einblenden",
        hide_graph_list: "Graphenliste ausblenden",
    },
    Labels {
        app_about: "Acerca de MixRelay",
        app_quit: "Salir de MixRelay",
        quit: "Salir",
        workspace_file: "Archivo",
        workspace_new: "Nuevo espacio de trabajo",
        workspace_open: "Abrir…",
        workspace_save: "Guardar",
        workspace_save_as: "Guardar como…",
        view: "Visualización",
        midi_map: "Modo de mapeo MIDI",
        show_graph_list: "Mostrar lista de grafos",
        hide_graph_list: "Ocultar lista de grafos",
    },
    Labels {
        app_about: "Sobre o MixRelay",
        app_quit: "Sair do MixRelay",
        quit: "Sair",
        workspace_file: "Arquivo",
        workspace_new: "Novo espaço de trabalho",
        workspace_open: "Abrir…",
        workspace_save: "Salvar",
        workspace_save_as: "Salvar como…",
        view: "Visualizar",
        midi_map: "Modo de mapeamento MIDI",
        show_graph_list: "Mostrar lista de grafos",
        hide_graph_list: "Ocultar lista de grafos",
    },
    Labels {
        app_about: "Informazioni su MixRelay",
        app_quit: "Esci da MixRelay",
        quit: "Esci",
        workspace_file: "File",
        workspace_new: "Nuovo spazio di lavoro",
        workspace_open: "Apri…",
        workspace_save: "Salva",
        workspace_save_as: "Salva con nome…",
        view: "Vista",
        midi_map: "Modalità mappatura MIDI",
        show_graph_list: "Mostra elenco grafi",
        hide_graph_list: "Nascondi elenco grafi",
    },
    Labels {
        app_about: "О MixRelay",
        app_quit: "Выйти из MixRelay",
        quit: "Выйти",
        workspace_file: "Файл",
        workspace_new: "Новое рабочее пространство",
        workspace_open: "Открыть…",
        workspace_save: "Сохранить",
        workspace_save_as: "Сохранить как…",
        view: "Вид",
        midi_map: "Режим MIDI-назначений",
        show_graph_list: "Показать список графов",
        hide_graph_list: "Скрыть список графов",
    },
];

/// Matches the UI locale, including Chinese region and script aliases.
pub(super) fn labels(locale: &str) -> Labels {
    LABELS[crate::native_text::resolve_locale(locale).index()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_supported_menus_have_translated_actions_and_chinese_aliases() {
        for (tag, file, quit) in [
            ("en", "File", "Quit"),
            ("ko-KR", "파일", "종료"),
            ("ja-JP", "ファイル", "終了"),
            ("zh-CN", "文件", "退出"),
            ("zh-HK", "檔案", "結束"),
            ("fr-FR", "Fichier", "Quitter"),
            ("de-DE", "Datei", "Beenden"),
            ("es-ES", "Archivo", "Salir"),
            ("pt-BR", "Arquivo", "Sair"),
            ("it-IT", "File", "Esci"),
            ("ru-RU", "Файл", "Выйти"),
        ] {
            let text = labels(tag);
            assert_eq!(text.workspace_file, file);
            assert_eq!(text.quit, quit);
            assert!(text.app_quit.contains("MixRelay"));
            assert!(text.app_about.contains("MixRelay"));
            for action in [
                text.workspace_new,
                text.workspace_open,
                text.workspace_save,
                text.workspace_save_as,
                text.view,
                text.show_graph_list,
                text.hide_graph_list,
            ] {
                assert!(!action.is_empty());
            }
            assert_ne!(text.show_graph_list, text.hide_graph_list);
        }
        assert_eq!(labels("zh-Hans-TW").workspace_file, "文件");
        assert_eq!(labels("zh-Hant-CN").workspace_file, "檔案");
    }

    #[test]
    fn korean_menu_uses_concise_action_labels() {
        let korean = labels("ko-KR");
        assert_eq!(korean.app_about, "MixRelay 정보");
        assert_eq!(korean.quit, "종료");
        assert_eq!(korean.workspace_file, "파일");
        assert_eq!(korean.workspace_new, "새 작업 공간");
        assert_eq!(korean.workspace_open, "열기…");
        assert_eq!(korean.workspace_save, "저장");
        assert_eq!(korean.workspace_save_as, "다른 이름으로 저장…");

        assert_eq!(labels("ko").workspace_file, korean.workspace_file);
    }

    #[test]
    fn other_locales_use_english_like_the_ui() {
        for locale in ["en", "en-US", "pseudo", ""] {
            let english = labels(locale);
            assert_eq!(english.app_about, "About MixRelay");
            assert_eq!(english.quit, "Quit");
            assert_eq!(english.workspace_file, "File");
            assert_eq!(english.workspace_new, "New Workspace");
            assert_eq!(english.workspace_open, "Open…");
            assert_eq!(english.workspace_save, "Save");
            assert_eq!(english.workspace_save_as, "Save As…");
        }
    }

    #[test]
    fn midi_menu_uses_each_locales_own_translation() {
        for (locale, expected) in [
            ("fr-FR", "Mode de mapping MIDI"),
            ("de-DE", "MIDI-Zuweisungsmodus"),
            ("es-ES", "Modo de mapeo MIDI"),
            ("pt-BR", "Modo de mapeamento MIDI"),
            ("it-IT", "Modalità mappatura MIDI"),
        ] {
            assert_eq!(labels(locale).midi_map, expected, "{locale}");
        }
    }
}
