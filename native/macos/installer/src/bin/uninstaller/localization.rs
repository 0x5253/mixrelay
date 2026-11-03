#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UiLocale {
    English,
    Korean,
    Japanese,
    SimplifiedChinese,
    TraditionalChinese,
    French,
    German,
    Spanish,
    Portuguese,
    Italian,
    Russian,
}

impl UiLocale {
    pub(super) fn from_preferences<'a>(languages: impl IntoIterator<Item = &'a str>) -> Self {
        languages
            .into_iter()
            .find_map(Self::supported)
            .unwrap_or(Self::English)
    }

    fn supported(language: &str) -> Option<Self> {
        let language = language.trim().replace('_', "-").to_ascii_lowercase();
        let parts: Vec<_> = language.split('-').collect();
        match parts.first().copied()? {
            "en" => Some(Self::English),
            "ko" => Some(Self::Korean),
            "ja" => Some(Self::Japanese),
            "fr" => Some(Self::French),
            "de" => Some(Self::German),
            "es" => Some(Self::Spanish),
            "pt" => Some(Self::Portuguese),
            "it" => Some(Self::Italian),
            "ru" => Some(Self::Russian),
            "zh" => Some(if parts.contains(&"hant") {
                Self::TraditionalChinese
            } else if parts.contains(&"hans") {
                Self::SimplifiedChinese
            } else if parts.iter().any(|part| matches!(*part, "tw" | "hk" | "mo")) {
                Self::TraditionalChinese
            } else {
                Self::SimplifiedChinese
            }),
            _ => None,
        }
    }

    pub(super) fn text(self, text: Text) -> &'static str {
        let labels = match self {
            Self::English => &ENGLISH,
            Self::Korean => &KOREAN,
            Self::Japanese => &JAPANESE,
            Self::SimplifiedChinese => &SIMPLIFIED_CHINESE,
            Self::TraditionalChinese => &TRADITIONAL_CHINESE,
            Self::French => &FRENCH,
            Self::German => &GERMAN,
            Self::Spanish => &SPANISH,
            Self::Portuguese => &PORTUGUESE,
            Self::Italian => &ITALIAN,
            Self::Russian => &RUSSIAN,
        };
        labels[text as usize]
    }
}

#[derive(Clone, Copy)]
pub(super) enum Text {
    ConfirmTitle,
    ConfirmDetail,
    Cancel,
    Remove,
    DeleteSettings,
    Ok,
    NotInstalledTitle,
    NothingToRemove,
    RemovedTitle,
    SettingsPreserved,
    FailedTitle,
    Retry,
    DownloadAgain,
    Authorization,
    SettingsDeleted,
    Working,
}

const ENGLISH: [&str; 16] = [
    "Remove MixRelay?",
    "Removing MixRelay stops its devices and loopback and briefly interrupts system audio. Quit recording and call apps before continuing.\n\nSaved MixRelay settings are kept unless you choose to delete them.",
    "Cancel",
    "Remove MixRelay",
    "Delete saved MixRelay settings",
    "OK",
    "MixRelay is not installed.",
    "There is nothing to remove.",
    "MixRelay removed.",
    "Saved MixRelay settings were kept.",
    "MixRelay could not be removed.",
    "Could not confirm that removal completed. Some components may already have been removed. Check the current installation before retrying. If the problem continues, contact support@rawsector.com.",
    "The removal app could not be verified. Download it again from the official source and retry.",
    "Administrator authentication could not be completed. Try again and approve the authentication request if prompted. If the problem continues, contact support@rawsector.com.",
    "Saved MixRelay settings were also deleted.",
    "Removing MixRelay…",
];
const KOREAN: [&str; 16] = [
    "MixRelay를 제거하시겠습니까?",
    "MixRelay 장치와 루프백이 중지되며 시스템 오디오가 잠시 끊깁니다. 계속하기 전에 녹음과 통화 앱을 종료하십시오.\n\n저장된 MixRelay 설정은 삭제 옵션을 선택하지 않으면 보존합니다.",
    "취소",
    "MixRelay 제거",
    "저장된 MixRelay 설정 삭제",
    "확인",
    "MixRelay가 설치되어 있지 않습니다.",
    "제거할 항목이 없습니다.",
    "MixRelay를 제거했습니다.",
    "저장된 MixRelay 설정은 보존했습니다.",
    "MixRelay를 제거하지 못했습니다.",
    "제거 완료를 확인하지 못했습니다. 일부 구성 요소는 이미 제거되었을 수 있습니다. 다시 시도하기 전에 현재 설치 상태를 확인하십시오. 문제가 계속되면 support@rawsector.com으로 문의하십시오.",
    "제거 앱을 확인하지 못했습니다. 공식 배포 경로에서 다시 내려받아 실행하십시오.",
    "관리자 인증을 완료하지 못했습니다. 다시 시도하고 관리자 인증 요청이 표시되면 승인하십시오. 문제가 계속되면 support@rawsector.com으로 문의하십시오.",
    "저장된 MixRelay 설정도 삭제했습니다.",
    "MixRelay를 제거하고 있습니다…",
];
const JAPANESE: [&str; 16] = [
    "MixRelayを削除しますか？",
    "MixRelayを削除すると、デバイスとループバックが停止し、システム音声が一時的に中断します。続行する前に録音・通話アプリを終了してください。\n\n削除オプションを選択しない限り、MixRelayの保存済み設定は保持されます。",
    "キャンセル",
    "MixRelayを削除",
    "MixRelayの保存済み設定を削除",
    "OK",
    "MixRelayはインストールされていません。",
    "削除する項目はありません。",
    "MixRelayを削除しました。",
    "MixRelayの保存済み設定は保持されました。",
    "MixRelayを削除できませんでした。",
    "削除の完了を確認できませんでした。一部のコンポーネントはすでに削除されている可能性があります。再試行する前に現在のインストール状態を確認してください。問題が続く場合はsupport@rawsector.comへお問い合わせください。",
    "削除アプリを検証できませんでした。公式の配布元から再ダウンロードして再実行してください。",
    "管理者認証を完了できませんでした。再試行し、認証を求められたら承認してください。問題が続く場合はsupport@rawsector.comへお問い合わせください。",
    "MixRelayの保存済み設定も削除されました。",
    "MixRelayを削除しています…",
];
const SIMPLIFIED_CHINESE: [&str; 16] = [
    "移除 MixRelay？",
    "移除 MixRelay 将停止其设备和回环，并暂时中断系统音频。继续之前，请退出录音和通话应用。\n\n除非您选择删除设置，否则将保留已保存的 MixRelay 设置。",
    "取消",
    "移除 MixRelay",
    "删除已保存的 MixRelay 设置",
    "确定",
    "未安装 MixRelay。",
    "没有需要移除的项目。",
    "已移除 MixRelay。",
    "已保留保存的 MixRelay 设置。",
    "无法移除 MixRelay。",
    "无法确认移除是否完成。部分组件可能已被移除。重试前请检查当前安装状态。如果问题持续，请联系 support@rawsector.com。",
    "无法验证移除应用。请从官方来源重新下载并重试。",
    "无法完成管理员认证。请重试，并在出现认证提示时批准。如果问题持续，请联系 support@rawsector.com。",
    "已同时删除保存的 MixRelay 设置。",
    "正在移除 MixRelay…",
];
const TRADITIONAL_CHINESE: [&str; 16] = [
    "移除 MixRelay？",
    "移除 MixRelay 將停止其裝置和回送，並暫時中斷系統音訊。繼續之前，請結束錄音及通話應用程式。\n\n除非您選擇刪除設定，否則將保留已儲存的 MixRelay 設定。",
    "取消",
    "移除 MixRelay",
    "刪除已儲存的 MixRelay 設定",
    "確定",
    "未安裝 MixRelay。",
    "沒有需要移除的項目。",
    "已移除 MixRelay。",
    "已保留儲存的 MixRelay 設定。",
    "無法移除 MixRelay。",
    "無法確認移除是否完成。部分元件可能已被移除。重試前請檢查目前安裝狀態。如果問題持續，請聯絡 support@rawsector.com。",
    "無法驗證移除應用程式。請從官方來源重新下載並重試。",
    "無法完成管理員認證。請重試，並在出現認證提示時核准。如果問題持續，請聯絡 support@rawsector.com。",
    "已同時刪除儲存的 MixRelay 設定。",
    "正在移除 MixRelay…",
];
const FRENCH: [&str; 16] = [
    "Supprimer MixRelay ?",
    "La suppression de MixRelay arrête ses périphériques et son bouclage audio, et interrompt brièvement l’audio du système. Fermez les applications d’enregistrement et d’appel avant de continuer.\n\nLes réglages enregistrés de MixRelay sont conservés, sauf si vous choisissez de les supprimer.",
    "Annuler",
    "Supprimer MixRelay",
    "Supprimer les réglages enregistrés de MixRelay",
    "OK",
    "MixRelay n’est pas installé.",
    "Il n’y a rien à supprimer.",
    "MixRelay a été supprimé.",
    "Les réglages enregistrés de MixRelay ont été conservés.",
    "Impossible de supprimer MixRelay.",
    "Impossible de confirmer la fin de la suppression. Certains composants ont peut-être déjà été supprimés. Vérifiez l’état actuel de l’installation avant de réessayer. Si le problème persiste, contactez support@rawsector.com.",
    "Impossible de vérifier l’application de suppression. Téléchargez-la à nouveau depuis la source officielle, puis réessayez.",
    "L’authentification administrateur n’a pas pu être effectuée. Réessayez et acceptez la demande d’authentification si elle s’affiche. Si le problème persiste, contactez support@rawsector.com.",
    "Les réglages enregistrés de MixRelay ont également été supprimés.",
    "Suppression de MixRelay en cours…",
];
const GERMAN: [&str; 16] = [
    "MixRelay entfernen?",
    "Das Entfernen von MixRelay beendet seine Geräte und das Loopback und unterbricht kurz das Systemaudio. Beenden Sie Aufnahme- und Anruf-Apps, bevor Sie fortfahren.\n\nDie gespeicherten Einstellungen von MixRelay bleiben erhalten, sofern Sie nicht deren Löschung auswählen.",
    "Abbrechen",
    "MixRelay entfernen",
    "Gespeicherte Einstellungen von MixRelay löschen",
    "OK",
    "MixRelay ist nicht installiert.",
    "Es gibt nichts zu entfernen.",
    "MixRelay wurde entfernt.",
    "Die gespeicherten Einstellungen von MixRelay wurden beibehalten.",
    "MixRelay konnte nicht entfernt werden.",
    "Der Abschluss der Entfernung konnte nicht bestätigt werden. Einige Komponenten wurden möglicherweise bereits entfernt. Prüfen Sie vor einem erneuten Versuch den aktuellen Installationsstatus. Wenn das Problem weiterhin besteht, wenden Sie sich an support@rawsector.com.",
    "Die Entfernungs-App konnte nicht verifiziert werden. Laden Sie sie erneut von der offiziellen Quelle herunter und versuchen Sie es noch einmal.",
    "Die Administratorauthentifizierung konnte nicht abgeschlossen werden. Versuchen Sie es erneut und bestätigen Sie die Authentifizierung, falls Sie dazu aufgefordert werden. Wenn das Problem weiterhin besteht, wenden Sie sich an support@rawsector.com.",
    "Die gespeicherten Einstellungen von MixRelay wurden ebenfalls gelöscht.",
    "MixRelay wird entfernt…",
];

const SPANISH: [&str; 16] = [
    "¿Eliminar MixRelay?",
    "Eliminar MixRelay detiene sus dispositivos y el bucle de audio e interrumpe brevemente el audio del sistema. Cierre las aplicaciones de grabación y llamadas antes de continuar.\n\nLa configuración guardada de MixRelay se conserva, salvo que elija eliminarla.",
    "Cancelar",
    "Eliminar MixRelay",
    "Eliminar la configuración guardada de MixRelay",
    "Aceptar",
    "MixRelay no está instalado.",
    "No hay nada que eliminar.",
    "MixRelay se ha eliminado.",
    "Se ha conservado la configuración guardada de MixRelay.",
    "No se pudo eliminar MixRelay.",
    "No se pudo confirmar que la eliminación haya terminado. Es posible que algunos componentes ya se hayan eliminado. Compruebe el estado actual de la instalación antes de reintentarlo. Si el problema continúa, contacte con support@rawsector.com.",
    "No se pudo verificar la aplicación de eliminación. Descárguela de nuevo de la fuente oficial y vuelva a intentarlo.",
    "No se pudo completar la autenticación del administrador. Vuelva a intentarlo y apruebe la autenticación si se le solicita. Si el problema continúa, contacte con support@rawsector.com.",
    "También se ha eliminado la configuración guardada de MixRelay.",
    "Eliminando MixRelay…",
];
const PORTUGUESE: [&str; 16] = [
    "Remover o MixRelay?",
    "A remoção do MixRelay interrompe seus dispositivos e o loopback, além de interromper brevemente o áudio do sistema. Feche os aplicativos de gravação e chamadas antes de continuar.\n\nAs configurações salvas do MixRelay são mantidas, a menos que você escolha excluí-las.",
    "Cancelar",
    "Remover MixRelay",
    "Excluir as configurações salvas do MixRelay",
    "OK",
    "O MixRelay não está instalado.",
    "Não há nada para remover.",
    "O MixRelay foi removido.",
    "As configurações salvas do MixRelay foram mantidas.",
    "Não foi possível remover o MixRelay.",
    "Não foi possível confirmar a conclusão da remoção. Alguns componentes podem já ter sido removidos. Verifique o estado atual da instalação antes de tentar novamente. Se o problema continuar, entre em contato com support@rawsector.com.",
    "Não foi possível verificar o aplicativo de remoção. Baixe-o novamente da fonte oficial e tente de novo.",
    "Não foi possível concluir a autenticação do administrador. Tente novamente e aprove a autenticação se ela for solicitada. Se o problema continuar, entre em contato com support@rawsector.com.",
    "As configurações salvas do MixRelay também foram excluídas.",
    "Removendo o MixRelay…",
];
const ITALIAN: [&str; 16] = [
    "Rimuovere MixRelay?",
    "La rimozione di MixRelay arresta i dispositivi e il loopback e interrompe brevemente l’audio del sistema. Chiudete le app di registrazione e chiamata prima di continuare.\n\nLe impostazioni salvate di MixRelay vengono conservate, a meno che non scegliate di eliminarle.",
    "Annulla",
    "Rimuovi MixRelay",
    "Elimina le impostazioni salvate di MixRelay",
    "OK",
    "MixRelay non è installato.",
    "Non c’è nulla da rimuovere.",
    "MixRelay è stato rimosso.",
    "Le impostazioni salvate di MixRelay sono state conservate.",
    "Impossibile rimuovere MixRelay.",
    "Impossibile confermare il completamento della rimozione. Alcuni componenti potrebbero essere già stati rimossi. Verificate lo stato attuale dell’installazione prima di riprovare. Se il problema persiste, contattate support@rawsector.com.",
    "Impossibile verificare l’app di rimozione. Scaricatela nuovamente dalla fonte ufficiale e riprovate.",
    "Impossibile completare l’autenticazione amministratore. Riprovate e approvate la richiesta di autenticazione, se visualizzata. Se il problema persiste, contattate support@rawsector.com.",
    "Anche le impostazioni salvate di MixRelay sono state eliminate.",
    "Rimozione di MixRelay in corso…",
];
const RUSSIAN: [&str; 16] = [
    "Удалить MixRelay?",
    "Удаление MixRelay остановит его устройства и loopback и ненадолго прервёт системный звук. Перед продолжением закройте приложения записи и звонков.\n\nСохранённые настройки MixRelay останутся, если вы не выберете их удаление.",
    "Отмена",
    "Удалить MixRelay",
    "Удалить сохранённые настройки MixRelay",
    "ОК",
    "MixRelay не установлен.",
    "Нет компонентов для удаления.",
    "MixRelay удалён.",
    "Сохранённые настройки MixRelay оставлены.",
    "Не удалось удалить MixRelay.",
    "Не удалось подтвердить завершение удаления. Некоторые компоненты, возможно, уже удалены. Перед повторной попыткой проверьте текущее состояние установки. Если проблема повторяется, обратитесь по адресу support@rawsector.com.",
    "Не удалось проверить приложение удаления. Снова загрузите его из официального источника и повторите попытку.",
    "Не удалось завершить проверку подлинности администратора. Повторите попытку и подтвердите запрос проверки, если он появится. Если проблема повторяется, обратитесь по адресу support@rawsector.com.",
    "Сохранённые настройки MixRelay также удалены.",
    "Удаление MixRelay…",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_first_supported_os_preference() {
        for (language, expected) in [
            ("en-GB", UiLocale::English),
            ("ko-KR", UiLocale::Korean),
            ("ja-JP", UiLocale::Japanese),
            ("zh-CN", UiLocale::SimplifiedChinese),
            ("zh-TW", UiLocale::TraditionalChinese),
            ("fr-CA", UiLocale::French),
            ("de-DE", UiLocale::German),
            ("es-MX", UiLocale::Spanish),
            ("pt-BR", UiLocale::Portuguese),
            ("it-IT", UiLocale::Italian),
            ("ru-RU", UiLocale::Russian),
        ] {
            assert_eq!(
                UiLocale::from_preferences(["ar-SA", language, "en"]),
                expected
            );
        }
        assert_eq!(UiLocale::from_preferences(["ar", "nl"]), UiLocale::English);
        assert_eq!(UiLocale::from_preferences([]), UiLocale::English);
        assert_eq!(UiLocale::from_preferences(["fr", "de"]), UiLocale::French);
    }

    #[test]
    fn chinese_scripts_override_regions_and_region_aliases_work() {
        for language in [
            "zh-TW",
            "zh-HK",
            "zh-MO",
            "zh-Hant",
            "zh-Hant-CN",
            "ZH_hant_cn",
        ] {
            assert_eq!(
                UiLocale::from_preferences([language]),
                UiLocale::TraditionalChinese
            );
        }
        for language in [
            "zh",
            "zh-CN",
            "zh-SG",
            "zh-Hans",
            "zh-Hans-TW",
            "ZH_hans_tw",
        ] {
            assert_eq!(
                UiLocale::from_preferences([language]),
                UiLocale::SimplifiedChinese
            );
        }
        assert_eq!(
            UiLocale::from_preferences(["koala", "ja"]),
            UiLocale::Japanese
        );
    }

    #[test]
    fn every_locale_preserves_removal_choices_and_recovery_contact() {
        for locale in [
            UiLocale::English,
            UiLocale::Korean,
            UiLocale::Japanese,
            UiLocale::SimplifiedChinese,
            UiLocale::TraditionalChinese,
            UiLocale::French,
            UiLocale::German,
            UiLocale::Spanish,
            UiLocale::Portuguese,
            UiLocale::Italian,
            UiLocale::Russian,
        ] {
            assert!(!locale.text(Text::Cancel).is_empty());
            assert!(!locale.text(Text::Remove).is_empty());
            assert!(locale.text(Text::DeleteSettings).contains("MixRelay"));
            assert!(!locale.text(Text::DeleteSettings).contains("Driver"));
            assert!(locale.text(Text::ConfirmDetail).contains("MixRelay"));
            assert!(locale.text(Text::Retry).contains("support@rawsector.com"));
            assert!(
                locale
                    .text(Text::Authorization)
                    .contains("support@rawsector.com")
            );
            let working = locale.text(Text::Working);
            assert!(working.contains("MixRelay"));
            assert!(working.ends_with('…'));
            assert_ne!(working, locale.text(Text::RemovedTitle));
            assert_ne!(working, locale.text(Text::FailedTitle));
        }
    }
}
