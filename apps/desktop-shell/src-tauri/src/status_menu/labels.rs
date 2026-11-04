//! Status menu text for the supported UI languages.

#[derive(Clone, Copy, Debug)]
pub(super) struct Labels {
    pub(super) open_editor: &'static str,
    pub(super) devices: &'static str,
    pub(super) channels: &'static str,
    pub(super) sample_rate: &'static str,
    pub(super) bit_depth: &'static str,
    pub(super) mixed: &'static str,
    pub(super) pause: &'static str,
    pub(super) resume: &'static str,
    pub(super) changing_audio: &'static str,
    pub(super) pause_failed: &'static str,
    pub(super) checking: &'static str,
    pub(super) check_updates: &'static str,
    pub(super) download: &'static str,
    pub(super) current: &'static str,
    pub(super) unpublished: &'static str,
    pub(super) update_failed: &'static str,
    pub(super) download_failed: &'static str,
    pub(super) quit: &'static str,
}

const LABELS: [Labels; 11] = [
    Labels {
        open_editor: "Open MixRelay",
        devices: "Devices",
        channels: "Channels",
        sample_rate: "Sample Rate",
        bit_depth: "Bit Depth",
        mixed: "Mixed",
        pause: "Pause",
        resume: "Resume",
        changing_audio: "Changing audio state…",
        pause_failed: "Could not pause or resume audio",
        checking: "Checking…",
        check_updates: "Check for Updates",
        download: "Download…",
        current: "Up to Date",
        unpublished: "No Published Updates",
        update_failed: "Update Check Failed",
        download_failed: "Download Link Failed",
        quit: "Quit",
    },
    Labels {
        open_editor: "MixRelay 열기",
        devices: "장치",
        channels: "채널 수",
        sample_rate: "샘플레이트",
        bit_depth: "비트 깊이",
        mixed: "혼합",
        pause: "일시정지",
        resume: "재개",
        changing_audio: "오디오 상태 변경 중…",
        pause_failed: "오디오 일시정지 또는 재개에 실패했습니다",
        checking: "확인 중…",
        check_updates: "업데이트 확인",
        download: "다운로드…",
        current: "최신 버전입니다",
        unpublished: "배포된 업데이트 없음",
        update_failed: "업데이트 확인 실패",
        download_failed: "다운로드 링크 열기 실패",
        quit: "종료",
    },
    Labels {
        open_editor: "MixRelayを開く",
        devices: "デバイス",
        channels: "チャンネル数",
        sample_rate: "サンプルレート",
        bit_depth: "ビット深度",
        mixed: "混在",
        pause: "一時停止",
        resume: "再開",
        changing_audio: "オーディオの状態を変更中…",
        pause_failed: "オーディオの一時停止または再開に失敗しました",
        checking: "確認中…",
        check_updates: "アップデートを確認",
        download: "ダウンロード…",
        current: "最新です",
        unpublished: "公開されたアップデートはありません",
        update_failed: "アップデートの確認に失敗",
        download_failed: "ダウンロードリンクを開けませんでした",
        quit: "終了",
    },
    Labels {
        open_editor: "打开 MixRelay",
        devices: "设备",
        channels: "声道数",
        sample_rate: "采样率",
        bit_depth: "位深度",
        mixed: "混合",
        pause: "暂停",
        resume: "继续",
        changing_audio: "正在更改音频状态…",
        pause_failed: "音频暂停或继续失败",
        checking: "正在检查…",
        check_updates: "检查更新",
        download: "下载…",
        current: "已是最新版本",
        unpublished: "没有已发布的更新",
        update_failed: "检查更新失败",
        download_failed: "无法打开下载链接",
        quit: "退出",
    },
    Labels {
        open_editor: "開啟 MixRelay",
        devices: "裝置",
        channels: "聲道數",
        sample_rate: "取樣率",
        bit_depth: "位元深度",
        mixed: "混合",
        pause: "暫停",
        resume: "繼續",
        changing_audio: "正在變更音訊狀態…",
        pause_failed: "音訊暫停或繼續失敗",
        checking: "正在檢查…",
        check_updates: "檢查更新",
        download: "下載…",
        current: "已是最新版本",
        unpublished: "沒有已發布的更新",
        update_failed: "檢查更新失敗",
        download_failed: "無法開啟下載連結",
        quit: "結束",
    },
    Labels {
        open_editor: "Ouvrir MixRelay",
        devices: "Périphériques",
        channels: "Canaux",
        sample_rate: "Fréquence d’échantillonnage",
        bit_depth: "Profondeur de bits",
        mixed: "Mixte",
        pause: "Mettre en pause",
        resume: "Reprendre",
        changing_audio: "Changement de l’état audio…",
        pause_failed: "Impossible de mettre l’audio en pause ou de le reprendre",
        checking: "Vérification…",
        check_updates: "Rechercher des mises à jour",
        download: "Télécharger…",
        current: "À jour",
        unpublished: "Aucune mise à jour publiée",
        update_failed: "Échec de la vérification des mises à jour",
        download_failed: "Impossible d’ouvrir le lien de téléchargement",
        quit: "Quitter",
    },
    Labels {
        open_editor: "MixRelay öffnen",
        devices: "Geräte",
        channels: "Kanäle",
        sample_rate: "Abtastrate",
        bit_depth: "Bittiefe",
        mixed: "Gemischt",
        pause: "Pausieren",
        resume: "Fortsetzen",
        changing_audio: "Audiostatus wird geändert…",
        pause_failed: "Audio konnte nicht pausiert oder fortgesetzt werden",
        checking: "Wird geprüft…",
        check_updates: "Nach Updates suchen",
        download: "Herunterladen…",
        current: "Aktuell",
        unpublished: "Keine veröffentlichten Updates",
        update_failed: "Updateprüfung fehlgeschlagen",
        download_failed: "Downloadlink konnte nicht geöffnet werden",
        quit: "Beenden",
    },
    Labels {
        open_editor: "Abrir MixRelay",
        devices: "Dispositivos",
        channels: "Canales",
        sample_rate: "Frecuencia de muestreo",
        bit_depth: "Profundidad de bits",
        mixed: "Mixto",
        pause: "Pausar",
        resume: "Reanudar",
        changing_audio: "Cambiando el estado del audio…",
        pause_failed: "No se pudo pausar o reanudar el audio",
        checking: "Comprobando…",
        check_updates: "Buscar actualizaciones",
        download: "Descargar…",
        current: "Actualizado",
        unpublished: "No hay actualizaciones publicadas",
        update_failed: "Error al buscar actualizaciones",
        download_failed: "No se pudo abrir el enlace de descarga",
        quit: "Salir",
    },
    Labels {
        open_editor: "Abrir MixRelay",
        devices: "Dispositivos",
        channels: "Canais",
        sample_rate: "Taxa de amostragem",
        bit_depth: "Profundidade de bits",
        mixed: "Misto",
        pause: "Pausar",
        resume: "Retomar",
        changing_audio: "Alterando o estado do áudio…",
        pause_failed: "Não foi possível pausar ou retomar o áudio",
        checking: "A verificar…",
        check_updates: "Procurar atualizações",
        download: "Baixar…",
        current: "Atualizado",
        unpublished: "Não há atualizações publicadas",
        update_failed: "Falha ao verificar atualizações",
        download_failed: "Não foi possível abrir o link de download",
        quit: "Sair",
    },
    Labels {
        open_editor: "Apri MixRelay",
        devices: "Dispositivi",
        channels: "Canali",
        sample_rate: "Frequenza di campionamento",
        bit_depth: "Profondità di bit",
        mixed: "Misto",
        pause: "Pausa",
        resume: "Riprendi",
        changing_audio: "Cambio dello stato audio…",
        pause_failed: "Impossibile mettere in pausa o riprendere l’audio",
        checking: "Verifica…",
        check_updates: "Verifica aggiornamenti",
        download: "Scarica…",
        current: "Versione aggiornata",
        unpublished: "Nessun aggiornamento pubblicato",
        update_failed: "Verifica aggiornamenti non riuscita",
        download_failed: "Impossibile aprire il link di download",
        quit: "Esci",
    },
    Labels {
        open_editor: "Открыть MixRelay",
        devices: "Устройства",
        channels: "Каналы",
        sample_rate: "Частота дискретизации",
        bit_depth: "Разрядность",
        mixed: "Смешанный",
        pause: "Приостановить",
        resume: "Возобновить",
        changing_audio: "Изменение состояния аудио…",
        pause_failed: "Не удалось приостановить или возобновить аудио",
        checking: "Проверка…",
        check_updates: "Проверить обновления",
        download: "Скачать…",
        current: "Последняя версия",
        unpublished: "Опубликованных обновлений нет",
        update_failed: "Не удалось проверить обновления",
        download_failed: "Не удалось открыть ссылку загрузки",
        quit: "Выйти",
    },
];

/// Matches the UI locale, including Chinese region and script aliases.
pub(super) fn labels(locale: &str) -> Labels {
    LABELS[crate::native_text::resolve_locale(locale).index()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(text: &Labels) -> [&'static str; 18] {
        [
            text.open_editor,
            text.devices,
            text.channels,
            text.sample_rate,
            text.bit_depth,
            text.mixed,
            text.pause,
            text.resume,
            text.changing_audio,
            text.pause_failed,
            text.checking,
            text.check_updates,
            text.download,
            text.current,
            text.unpublished,
            text.update_failed,
            text.download_failed,
            text.quit,
        ]
    }

    #[test]
    fn every_supported_language_has_its_own_complete_status_menu_text() {
        let english = all(&labels("en"));
        for (locale, channels) in [
            ("ko", "채널 수"),
            ("ja", "チャンネル数"),
            ("zh-Hans", "声道数"),
            ("zh-Hant", "聲道數"),
            ("fr", "Canaux"),
            ("de", "Kanäle"),
            ("es", "Canales"),
            ("pt", "Canais"),
            ("it", "Canali"),
            ("ru", "Каналы"),
        ] {
            let text = labels(locale);
            assert_eq!(text.channels, channels);
            assert_ne!(text.quit, "Quit", "{locale}");
            for (message, english) in all(&text).into_iter().zip(english) {
                assert!(!message.is_empty(), "{locale}");
                // Product names and shared loanwords may match; sentences must not.
                if english.len() > 12 && !english.contains("MixRelay") {
                    assert_ne!(message, english, "{locale}");
                }
            }
            assert_ne!(text.pause, text.resume, "{locale}");
            assert_ne!(text.checking, text.check_updates, "{locale}");
        }
    }

    #[test]
    fn regional_and_script_tags_resolve_to_the_matching_translation() {
        assert_eq!(labels("ko-KR").quit, "종료");
        assert_eq!(labels("ja-JP").check_updates, "アップデートを確認");
        assert_eq!(labels("zh-TW").sample_rate, "取樣率");
        assert_eq!(labels("zh-SG").sample_rate, "采样率");
        assert_eq!(labels("pseudo").open_editor, "Open MixRelay");
    }

    #[test]
    fn korean_names_device_settings_and_plain_quit() {
        let ko = labels("ko-KR");
        assert_eq!(
            (
                ko.quit,
                ko.devices,
                ko.channels,
                ko.sample_rate,
                ko.bit_depth
            ),
            ("종료", "장치", "채널 수", "샘플레이트", "비트 깊이")
        );
        assert_eq!((ko.pause, ko.resume), ("일시정지", "재개"));
    }
}
