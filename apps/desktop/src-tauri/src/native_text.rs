//! Native desktop text follows the UI's resolved language.
use std::sync::atomic::{AtomicU8, Ordering};

pub(crate) mod dialogs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Locale {
    En,
    Ko,
    Ja,
    ZhHans,
    ZhHant,
    Fr,
    De,
    Es,
    Pt,
    It,
    Ru,
}

impl Locale {
    pub(crate) fn index(self) -> usize {
        usize::from(self as u8)
    }

    pub(crate) fn tag(self) -> &'static str {
        [
            "en", "ko", "ja", "zh-Hans", "zh-Hant", "fr", "de", "es", "pt", "it", "ru",
        ][self.index()]
    }
}

static CURRENT_LOCALE: AtomicU8 = AtomicU8::new(Locale::En as u8);

pub(crate) fn resolve_locale(language: &str) -> Locale {
    let normalized = language.trim().replace('_', "-").to_ascii_lowercase();
    let parts: Vec<_> = normalized.split('-').collect();
    match parts.first().copied() {
        Some("ko") => Locale::Ko,
        Some("ja") => Locale::Ja,
        Some("fr") => Locale::Fr,
        Some("de") => Locale::De,
        Some("es") => Locale::Es,
        Some("pt") => Locale::Pt,
        Some("it") => Locale::It,
        Some("ru") => Locale::Ru,
        Some("zh") if parts.contains(&"hant") => Locale::ZhHant,
        Some("zh") if parts.contains(&"hans") => Locale::ZhHans,
        Some("zh") if parts.iter().any(|part| ["tw", "hk", "mo"].contains(part)) => Locale::ZhHant,
        Some("zh") => Locale::ZhHans,
        _ => Locale::En,
    }
}

pub(crate) fn set_locale(language: &str) {
    CURRENT_LOCALE.store(resolve_locale(language) as u8, Ordering::Relaxed);
}

pub(crate) fn current_locale() -> Locale {
    match CURRENT_LOCALE.load(Ordering::Relaxed) {
        1 => Locale::Ko,
        2 => Locale::Ja,
        3 => Locale::ZhHans,
        4 => Locale::ZhHant,
        5 => Locale::Fr,
        6 => Locale::De,
        7 => Locale::Es,
        8 => Locale::Pt,
        9 => Locale::It,
        10 => Locale::Ru,
        _ => Locale::En,
    }
}

pub(crate) fn quit_failed(locale: Locale) -> (&'static str, &'static str) {
    const TEXT: [(&str, &str); 11] = [
        (
            "Could Not Stop MixRelay",
            "The devices and loopback could not be stopped, or the stopped state could not be saved. MixRelay has not quit. Try Quit again from the status menu shortly.",
        ),
        (
            "MixRelay를 종료하지 못했습니다",
            "장치와 루프백을 중지하거나 종료 상태를 저장하지 못했습니다. MixRelay는 아직 종료되지 않았습니다. 잠시 후 상태 메뉴에서 종료를 다시 선택하십시오.",
        ),
        (
            "MixRelayを終了できませんでした",
            "デバイスとループバックを停止するか、停止状態を保存することができませんでした。MixRelayはまだ終了していません。しばらくしてからステータスメニューの終了をもう一度選択してください。",
        ),
        (
            "无法停止 MixRelay",
            "无法停止设备和回环，或无法保存停止状态。MixRelay 尚未退出。请稍后在状态菜单中再次选择退出。",
        ),
        (
            "無法停止 MixRelay",
            "無法停止裝置和回送，或無法儲存停止狀態。MixRelay 尚未結束。請稍後在狀態選單中再次選擇結束。",
        ),
        (
            "Impossible d’arrêter MixRelay",
            "Les périphériques et le bouclage n’ont pas pu être arrêtés, ou leur état d’arrêt n’a pas pu être enregistré. La fermeture de MixRelay n’est pas terminée. Réessayez de quitter depuis le menu d’état dans un instant.",
        ),
        (
            "MixRelay konnte nicht beendet werden",
            "Die Geräte und das Loopback konnten nicht gestoppt werden oder der Stoppzustand konnte nicht gespeichert werden. MixRelay wurde noch nicht beendet. Wählen Sie gleich noch einmal Beenden im Statusmenü.",
        ),
        (
            "No se pudo detener MixRelay",
            "No se pudieron detener los dispositivos y el bucle de retorno, o no se pudo guardar el estado detenido. MixRelay aún no ha salido. Vuelva a elegir Salir en el menú de estado dentro de un momento.",
        ),
        (
            "Não foi possível parar o MixRelay",
            "Não foi possível parar os dispositivos e o loopback ou salvar o estado de parada. O MixRelay ainda não foi encerrado. Tente escolher Sair novamente no menu de estado dentro de instantes.",
        ),
        (
            "Impossibile fermare MixRelay",
            "Impossibile fermare i dispositivi e il loopback oppure salvare lo stato di arresto. MixRelay non è ancora uscito. Tra poco scegli di nuovo Esci dal menu di stato.",
        ),
        (
            "Не удалось остановить MixRelay",
            "Не удалось остановить устройства и loopback или сохранить состояние остановки. MixRelay ещё не завершился. Через некоторое время снова выберите «Выйти» в меню состояния.",
        ),
    ];
    TEXT[locale.index()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_locale_resolves_regions_scripts_and_unsupported_fallback() {
        for (tag, expected) in [
            ("en-US", Locale::En),
            ("ko", Locale::Ko),
            ("KO_kr", Locale::Ko),
            ("ja-JP", Locale::Ja),
            ("fr-CA", Locale::Fr),
            ("de-DE", Locale::De),
            ("es-MX", Locale::Es),
            ("pt-BR", Locale::Pt),
            ("pt-PT", Locale::Pt),
            ("it-IT", Locale::It),
            ("ru-RU", Locale::Ru),
            ("zh-TW", Locale::ZhHant),
            ("zh-HK", Locale::ZhHant),
            ("zh-MO", Locale::ZhHant),
            ("zh-Hant-CN", Locale::ZhHant),
            ("zh-Hans-TW", Locale::ZhHans),
            ("zh-CN", Locale::ZhHans),
            ("zh-SG", Locale::ZhHans),
            ("zh", Locale::ZhHans),
            ("franken", Locale::En),
            ("unknown", Locale::En),
            ("", Locale::En),
        ] {
            assert_eq!(resolve_locale(tag), expected, "{tag}");
        }
    }

    #[test]
    fn quit_failure_text_is_complete_and_has_no_internal_diagnostic_details() {
        for tag in [
            "ko-KR", "ja", "zh-Hans", "zh-Hant", "fr", "de", "es", "pt", "it", "ru",
        ] {
            let (title, body) = quit_failed(resolve_locale(tag));
            assert!(!title.is_empty());
            assert!(!body.is_empty());
            assert_ne!(quit_failed(Locale::En), (title, body));
            assert!(!body.contains("HTTP"));
            assert!(!body.contains("19342"));
        }
    }
}
