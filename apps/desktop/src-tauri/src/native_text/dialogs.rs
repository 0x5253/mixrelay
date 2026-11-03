//! Native dialog text. Runtime flow and diagnostic details remain in their callers.
use super::Locale;

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub(crate) enum Text {
    ProcessorFolder = 0,
    RecordingFolder = 1,
    RecordingFolderUnavailable = 2,
    RecordingFolderUnicode = 3,
    WorkspaceReplacePrompt = 4,
    WorkspaceReplace = 5,
    Cancel = 6,
    WorkspaceOpen = 7,
    WorkspaceSave = 8,
    WorkspaceLockFailed = 9,
    MainWindowUnavailable = 10,
    Ok = 11,
    EngineStoppingTitle = 12,
    #[cfg(any(target_os = "macos", test))]
    DesktopOwnershipFailed = 13,
    #[cfg(any(target_os = "macos", test))]
    AudioServerShuttingDownRetry = 14,
    #[cfg(any(target_os = "macos", test))]
    DesktopLifecycleLockFailed = 15,
    #[cfg(any(target_os = "macos", test))]
    AudioServiceInvalid = 16,
    #[cfg(any(target_os = "macos", test))]
    AudioServiceIncompatible = 17,
    #[cfg(any(target_os = "macos", test))]
    AudioServerShutdownWarning = 18,
    #[cfg(any(target_os = "linux", test))]
    BundledEngineStartFailed = 19,
    #[cfg(any(target_os = "linux", test))]
    DesktopExecutableParentMissing = 20,
    #[cfg(any(target_os = "linux", test))]
    BundledEngineMissing = 21,
    #[cfg(any(target_os = "linux", test))]
    EngineOwnershipLockFailed = 22,
    #[cfg(any(target_os = "linux", test))]
    PreviousEngineRunning = 23,
    #[cfg(any(target_os = "linux", test))]
    EngineFinalizationFailed = 24,
}

// Explicit indexes keep platform-specific variants aligned with these rows.
const TEXT: [[&str; 11]; 25] = [
    // ProcessorFolder
    [
        "Choose audio plug-in folder",
        "오디오 플러그인 폴더 선택",
        "オーディオプラグインのフォルダーを選択",
        "选择音频插件文件夹",
        "選擇音訊外掛程式資料夾",
        "Choisir le dossier de plug-ins audio",
        "Audio-Plug-in-Ordner auswählen",
        "Elegir carpeta de plug-ins de audio",
        "Escolher pasta de plug-ins de áudio",
        "Scegli cartella dei plug-in audio",
        "Выбрать папку аудиоплагинов",
    ],
    // RecordingFolder
    [
        "Choose recording folder",
        "녹음 폴더 선택",
        "録音フォルダーを選択",
        "选择录音文件夹",
        "選擇錄音資料夾",
        "Choisir le dossier d’enregistrement",
        "Aufnahmeordner auswählen",
        "Elegir carpeta de grabación",
        "Escolher pasta de gravação",
        "Scegli cartella di registrazione",
        "Выбрать папку для записи",
    ],
    // RecordingFolderUnavailable
    [
        "No existing user folder is available for recording",
        "녹음에 사용할 수 있는 기존 사용자 폴더가 없습니다",
        "録音に使用できる既存のユーザーフォルダーがありません",
        "没有可用于录音的现有用户文件夹",
        "沒有可用於錄音的現有使用者資料夾",
        "Aucun dossier utilisateur existant n’est disponible pour l’enregistrement",
        "Kein vorhandener Benutzerordner ist für Aufnahmen verfügbar",
        "No hay ninguna carpeta de usuario existente disponible para grabar",
        "Não existe uma pasta de usuário disponível para gravação",
        "Non è disponibile alcuna cartella utente esistente per la registrazione",
        "Нет существующей пользовательской папки, доступной для записи",
    ],
    // RecordingFolderUnicode
    [
        "Recording folder must be valid Unicode",
        "녹음 폴더 경로는 유효한 유니코드여야 합니다",
        "録音フォルダーのパスは有効な Unicode である必要があります",
        "录音文件夹路径必须是有效的 Unicode",
        "錄音資料夾路徑必須是有效的 Unicode",
        "Le chemin du dossier d’enregistrement doit être un texte Unicode valide",
        "Der Pfad des Aufnahmeordners muss gültigen Unicode enthalten",
        "La ruta de la carpeta de grabación debe ser Unicode válido",
        "O caminho da pasta de gravação deve conter Unicode válido",
        "Il percorso della cartella di registrazione deve contenere Unicode valido",
        "Путь к папке для записи должен содержать допустимые символы Unicode",
    ],
    // WorkspaceReplacePrompt
    [
        "Replace the current workspace? Cancel and save first to keep your work.",
        "현재 작업 공간을 교체할까요? 보관하려면 취소한 뒤 먼저 저장하세요.",
        "現在のワークスペースを置き換えますか？作業内容を残すには、キャンセルして先に保存してください。",
        "替换当前工作区？如需保留工作内容，请取消并先保存。",
        "取代目前的工作區？如需保留工作內容，請取消並先儲存。",
        "Remplacer l’espace de travail actuel ? Pour conserver votre travail, annulez et enregistrez d’abord.",
        "Den aktuellen Arbeitsbereich ersetzen? Zum Behalten Ihrer Arbeit zuerst abbrechen und speichern.",
        "¿Reemplazar el espacio de trabajo actual? Para conservar su trabajo, cancele y guárdelo primero.",
        "Substituir o espaço de trabalho atual? Para preservar o seu trabalho, cancele e salve primeiro.",
        "Sostituire lo spazio di lavoro attuale? Per conservare il lavoro, annulla e salva prima.",
        "Заменить текущую рабочую область? Чтобы сохранить свою работу, отмените действие и сначала сохраните её.",
    ],
    // WorkspaceReplace
    [
        "Replace",
        "교체",
        "置き換える",
        "替换",
        "取代",
        "Remplacer",
        "Ersetzen",
        "Reemplazar",
        "Substituir",
        "Sostituisci",
        "Заменить",
    ],
    // Cancel
    [
        "Cancel",
        "취소",
        "キャンセル",
        "取消",
        "取消",
        "Annuler",
        "Abbrechen",
        "Cancelar",
        "Cancelar",
        "Annulla",
        "Отмена",
    ],
    // WorkspaceOpen
    [
        "Open Workspace",
        "작업 공간 열기",
        "ワークスペースを開く",
        "打开工作区",
        "開啟工作區",
        "Ouvrir un espace de travail",
        "Arbeitsbereich öffnen",
        "Abrir espacio de trabajo",
        "Abrir espaço de trabalho",
        "Apri spazio di lavoro",
        "Открыть рабочую область",
    ],
    // WorkspaceSave
    [
        "Save Workspace",
        "작업 공간 저장",
        "ワークスペースを保存",
        "保存工作区",
        "儲存工作區",
        "Enregistrer l’espace de travail",
        "Arbeitsbereich speichern",
        "Guardar espacio de trabajo",
        "Salvar espaço de trabalho",
        "Salva spazio di lavoro",
        "Сохранить рабочую область",
    ],
    // WorkspaceLockFailed
    [
        "Workspace path lock failed",
        "작업 공간 경로 잠금에 실패했습니다",
        "ワークスペースのパスをロックできませんでした",
        "工作区路径锁定失败",
        "工作區路徑鎖定失敗",
        "Le verrouillage du chemin de l’espace de travail a échoué",
        "Der Arbeitsbereichspfad konnte nicht gesperrt werden",
        "No se pudo bloquear la ruta del espacio de trabajo",
        "Não foi possível bloquear o caminho do espaço de trabalho",
        "Impossibile bloccare il percorso dello spazio di lavoro",
        "Не удалось заблокировать путь рабочей области",
    ],
    // MainWindowUnavailable
    [
        "Main window unavailable",
        "기본 창을 사용할 수 없습니다",
        "メインウインドウを利用できません",
        "主窗口不可用",
        "無法使用主視窗",
        "La fenêtre principale est indisponible",
        "Das Hauptfenster ist nicht verfügbar",
        "La ventana principal no está disponible",
        "A janela principal não está disponível",
        "La finestra principale non è disponibile",
        "Главное окно недоступно",
    ],
    // Ok
    [
        "OK", "확인", "確認", "确定", "確定", "OK", "OK", "Aceptar", "OK", "OK", "ОК",
    ],
    // EngineStoppingTitle
    [
        "MixRelay — Shutting down audio server",
        "MixRelay — 오디오 서버 종료 중",
        "MixRelay — オーディオサーバーを終了中",
        "MixRelay — 正在关闭音频服务器",
        "MixRelay — 正在關閉音訊伺服器",
        "MixRelay — Arrêt du serveur audio",
        "MixRelay — Audioserver wird beendet",
        "MixRelay — Deteniendo el servidor de audio",
        "MixRelay — Encerrando o servidor de áudio",
        "MixRelay — Arresto del server audio",
        "MixRelay — Завершение работы аудиосервера",
    ],
    // DesktopOwnershipFailed
    [
        "Could not acquire desktop ownership",
        "데스크톱 앱의 소유권을 확보하지 못했습니다",
        "デスクトップアプリの所有権を取得できませんでした",
        "无法取得桌面应用的所有权",
        "無法取得桌面應用程式的擁有權",
        "Impossible d’obtenir la propriété de l’application de bureau",
        "Die Desktop-App konnte die Prozesshoheit nicht übernehmen",
        "No se pudo obtener el control de la aplicación de escritorio",
        "Não foi possível obter o controle da aplicação de desktop",
        "Impossibile ottenere il controllo dell’applicazione desktop",
        "Не удалось получить управление настольным приложением",
    ],
    // AudioServerShuttingDownRetry
    [
        "The audio server is still shutting down. Try opening MixRelay again shortly.",
        "오디오 서버가 아직 종료 중입니다. 잠시 후 MixRelay를 다시 열어 주세요.",
        "オーディオサーバーはまだ終了中です。しばらくしてから MixRelay をもう一度開いてください。",
        "音频服务器仍在关闭。请稍后重新打开 MixRelay。",
        "音訊伺服器仍在關閉。請稍後重新開啟 MixRelay。",
        "Le serveur audio est encore en cours d’arrêt. Réessayez d’ouvrir MixRelay dans un instant.",
        "Der Audioserver wird noch beendet. Versuchen Sie in Kürze, MixRelay erneut zu öffnen.",
        "El servidor de audio aún se está deteniendo. Intente abrir MixRelay de nuevo en unos instantes.",
        "O servidor de áudio ainda está sendo encerrado. Tente abrir o MixRelay novamente em instantes.",
        "Il server audio è ancora in fase di arresto. Riprova ad aprire MixRelay tra poco.",
        "Аудиосервер ещё завершает работу. Попробуйте снова открыть MixRelay немного позже.",
    ],
    // DesktopLifecycleLockFailed
    [
        "Desktop lifecycle ownership lock failed",
        "데스크톱 앱 수명 주기 소유권 잠금에 실패했습니다",
        "デスクトップアプリのライフサイクル所有権をロックできませんでした",
        "桌面应用生命周期所有权锁定失败",
        "桌面應用程式生命週期擁有權鎖定失敗",
        "Le verrouillage de la propriété du cycle de vie de l’application de bureau a échoué",
        "Die Prozesshoheit für den Lebenszyklus der Desktop-App konnte nicht gesperrt werden",
        "No se pudo bloquear el control del ciclo de vida de la aplicación de escritorio",
        "Não foi possível bloquear o controle do ciclo de vida da aplicação de desktop",
        "Impossibile bloccare il controllo del ciclo di vita dell’applicazione desktop",
        "Не удалось заблокировать управление жизненным циклом настольного приложения",
    ],
    // AudioServiceInvalid
    [
        "MixRelay audio service installation is missing or invalid.",
        "MixRelay 오디오 서비스가 설치되지 않았거나 설치가 올바르지 않습니다.",
        "MixRelay オーディオサービスがインストールされていないか、インストールが無効です。",
        "MixRelay 音频服务未安装或安装无效。",
        "MixRelay 音訊服務未安裝或安裝無效。",
        "L’installation du service audio MixRelay est absente ou invalide.",
        "Der MixRelay-Audiodienst fehlt oder ist nicht korrekt installiert.",
        "El servicio de audio de MixRelay no está instalado o su instalación no es válida.",
        "O serviço de áudio do MixRelay não está instalado ou a instalação é inválida.",
        "Il servizio audio di MixRelay non è installato o l’installazione non è valida.",
        "Аудиослужба MixRelay не установлена или установлена некорректно.",
    ],
    // AudioServiceIncompatible
    [
        "The MixRelay audio service is missing or incompatible. Reinstall MixRelay using its installer.",
        "MixRelay 오디오 서비스가 없거나 호환되지 않습니다. 설치 프로그램으로 MixRelay를 다시 설치하세요.",
        "MixRelay オーディオサービスが存在しないか、互換性がありません。インストーラーを使って MixRelay を再インストールしてください。",
        "MixRelay 音频服务缺失或不兼容。请使用安装程序重新安装 MixRelay。",
        "MixRelay 音訊服務遺失或不相容。請使用安裝程式重新安裝 MixRelay。",
        "Le service audio MixRelay est absent ou incompatible. Réinstallez MixRelay avec son programme d’installation.",
        "Der MixRelay-Audiodienst fehlt oder ist inkompatibel. Installieren Sie MixRelay mit dem Installationsprogramm erneut.",
        "El servicio de audio de MixRelay falta o no es compatible. Reinstale MixRelay con su instalador.",
        "O serviço de áudio do MixRelay está ausente ou é incompatível. Reinstale o MixRelay usando o instalador.",
        "Il servizio audio di MixRelay è assente o incompatibile. Reinstalla MixRelay con il suo programma di installazione.",
        "Аудиослужба MixRelay отсутствует или несовместима. Переустановите MixRelay с помощью установщика.",
    ],
    // AudioServerShutdownWarning
    [
        "An error occurred while shutting down the audio server.",
        "오디오 서버 종료 중 오류가 발생했습니다.",
        "オーディオサーバーの終了中にエラーが発生しました。",
        "关闭音频服务器时发生错误。",
        "關閉音訊伺服器時發生錯誤。",
        "Une erreur s’est produite pendant l’arrêt du serveur audio.",
        "Beim Beenden des Audioservers ist ein Fehler aufgetreten.",
        "Se produjo un error al detener el servidor de audio.",
        "Ocorreu um erro ao encerrar o servidor de áudio.",
        "Si è verificato un errore durante l’arresto del server audio.",
        "При завершении работы аудиосервера произошла ошибка.",
    ],
    // BundledEngineStartFailed
    [
        "Could not start the bundled audio engine",
        "포함된 오디오 엔진을 시작하지 못했습니다",
        "同梱のオーディオエンジンを起動できませんでした",
        "无法启动随附的音频引擎",
        "無法啟動隨附的音訊引擎",
        "Impossible de démarrer le moteur audio fourni",
        "Die mitgelieferte Audio-Engine konnte nicht gestartet werden",
        "No se pudo iniciar el motor de audio incluido",
        "Não foi possível iniciar o motor de áudio incluído",
        "Impossibile avviare il motore audio incluso",
        "Не удалось запустить встроенный аудиодвижок",
    ],
    // DesktopExecutableParentMissing
    [
        "Desktop executable has no parent directory",
        "데스크톱 실행 파일의 상위 디렉터리가 없습니다",
        "デスクトップ実行ファイルに親ディレクトリがありません",
        "桌面可执行文件没有父目录",
        "桌面執行檔沒有上層目錄",
        "L’exécutable de l’application de bureau n’a pas de dossier parent",
        "Die ausführbare Desktop-Datei hat kein übergeordnetes Verzeichnis",
        "El ejecutable de la aplicación de escritorio no tiene un directorio superior",
        "O executável da aplicação de desktop não tem um diretório superior",
        "L’eseguibile dell’applicazione desktop non ha una cartella superiore",
        "У исполняемого файла настольного приложения нет родительского каталога",
    ],
    // BundledEngineMissing
    [
        "Bundled audio engine is missing",
        "포함된 오디오 엔진이 없습니다",
        "同梱のオーディオエンジンがありません",
        "随附的音频引擎缺失",
        "隨附的音訊引擎遺失",
        "Le moteur audio fourni est absent",
        "Die mitgelieferte Audio-Engine fehlt",
        "Falta el motor de audio incluido",
        "O motor de áudio incluído está ausente",
        "Il motore audio incluso è assente",
        "Встроенный аудиодвижок отсутствует",
    ],
    // EngineOwnershipLockFailed
    [
        "Engine ownership lock failed",
        "엔진 소유권 잠금에 실패했습니다",
        "エンジンの所有権をロックできませんでした",
        "引擎所有权锁定失败",
        "引擎擁有權鎖定失敗",
        "Le verrouillage de la propriété du moteur a échoué",
        "Die Prozesshoheit für die Engine konnte nicht gesperrt werden",
        "No se pudo bloquear el control del motor",
        "Não foi possível bloquear o controle do motor",
        "Impossibile bloccare il controllo del motore",
        "Не удалось заблокировать управление аудиодвижком",
    ],
    // PreviousEngineRunning
    [
        "The previous audio engine is still running",
        "이전 오디오 엔진이 아직 실행 중입니다",
        "前のオーディオエンジンはまだ動作中です",
        "先前的音频引擎仍在运行",
        "先前的音訊引擎仍在執行",
        "Le moteur audio précédent fonctionne encore",
        "Die vorherige Audio-Engine läuft noch",
        "El motor de audio anterior aún está en ejecución",
        "O motor de áudio anterior ainda está em execução",
        "Il motore audio precedente è ancora in esecuzione",
        "Предыдущий аудиодвижок ещё работает",
    ],
    // EngineFinalizationFailed
    [
        "Audio engine stopped, but clean finalization failed",
        "오디오 엔진은 중지되었지만 정상적인 마무리에 실패했습니다",
        "オーディオエンジンは停止しましたが、正常に終了処理できませんでした",
        "音频引擎已停止，但正常收尾失败",
        "音訊引擎已停止，但正常結束處理失敗",
        "Le moteur audio s’est arrêté, mais sa finalisation normale a échoué",
        "Die Audio-Engine wurde beendet, konnte aber nicht ordnungsgemäß abgeschlossen werden",
        "El motor de audio se detuvo, pero no pudo finalizar correctamente",
        "O motor de áudio parou, mas não foi possível concluir corretamente o encerramento",
        "Il motore audio si è arrestato, ma la chiusura non è stata completata correttamente",
        "Аудиодвижок остановлен, но корректно завершить его работу не удалось",
    ],
];

pub(crate) fn text(locale: Locale, message: Text) -> &'static str {
    TEXT[message as usize][match locale {
        Locale::En => 0,
        Locale::Ko => 1,
        Locale::Ja => 2,
        Locale::ZhHans => 3,
        Locale::ZhHant => 4,
        Locale::Fr => 5,
        Locale::De => 6,
        Locale::Es => 7,
        Locale::Pt => 8,
        Locale::It => 9,
        Locale::Ru => 10,
    }]
}

pub(crate) fn detail(locale: Locale, message: Text, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", text(locale, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCALES: [Locale; 11] = [
        Locale::En,
        Locale::Ko,
        Locale::Ja,
        Locale::ZhHans,
        Locale::ZhHant,
        Locale::Fr,
        Locale::De,
        Locale::Es,
        Locale::Pt,
        Locale::It,
        Locale::Ru,
    ];

    const MESSAGES: &[Text] = &[
        Text::ProcessorFolder,
        Text::RecordingFolder,
        Text::RecordingFolderUnavailable,
        Text::RecordingFolderUnicode,
        Text::WorkspaceReplacePrompt,
        Text::WorkspaceReplace,
        Text::Cancel,
        Text::WorkspaceOpen,
        Text::WorkspaceSave,
        Text::WorkspaceLockFailed,
        Text::MainWindowUnavailable,
        Text::Ok,
        Text::EngineStoppingTitle,
        Text::DesktopOwnershipFailed,
        Text::AudioServerShuttingDownRetry,
        Text::DesktopLifecycleLockFailed,
        Text::AudioServiceInvalid,
        Text::AudioServiceIncompatible,
        Text::AudioServerShutdownWarning,
        Text::BundledEngineStartFailed,
        Text::DesktopExecutableParentMissing,
        Text::BundledEngineMissing,
        Text::EngineOwnershipLockFailed,
        Text::PreviousEngineRunning,
        Text::EngineFinalizationFailed,
    ];

    #[test]
    fn every_native_dialog_has_explicit_text_in_every_supported_language() {
        for locale in LOCALES {
            for message in MESSAGES {
                let value = text(locale, *message);
                assert!(!value.trim().is_empty(), "{locale:?} {message:?}");
                assert!(!value.contains("{{"), "unresolved interpolation: {value}");
            }
            assert_ne!(
                text(locale, Text::WorkspaceReplace),
                text(locale, Text::Cancel)
            );
        }
    }

    #[test]
    fn diagnostic_details_are_preserved_in_every_language() {
        let diagnostic = "mixrelay-service /tmp/声音: exit code 2";
        for locale in LOCALES {
            let message = detail(locale, Text::BundledEngineStartFailed, diagnostic);
            assert_eq!(
                message,
                format!(
                    "{}: {diagnostic}",
                    text(locale, Text::BundledEngineStartFailed)
                )
            );
        }
    }

    #[test]
    fn workspace_replacement_warning_preserves_cancel_and_save_instructions() {
        assert_eq!(
            text(Locale::En, Text::WorkspaceReplacePrompt),
            "Replace the current workspace? Cancel and save first to keep your work."
        );
        assert_eq!(
            text(Locale::Ko, Text::WorkspaceReplacePrompt),
            "현재 작업 공간을 교체할까요? 보관하려면 취소한 뒤 먼저 저장하세요."
        );
        assert!(text(Locale::Ja, Text::WorkspaceReplacePrompt).contains("先に保存"));
        assert!(text(Locale::ZhHans, Text::WorkspaceReplacePrompt).contains("先保存"));
        assert!(text(Locale::ZhHant, Text::WorkspaceReplacePrompt).contains("先儲存"));
        assert!(text(Locale::Fr, Text::WorkspaceReplacePrompt).contains("enregistrez d’abord"));
        assert!(
            text(Locale::De, Text::WorkspaceReplacePrompt)
                .contains("zuerst abbrechen und speichern")
        );
    }
}
