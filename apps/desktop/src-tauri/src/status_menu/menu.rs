use mixrelay_audio_io::VirtualEndpointDeviceSettings;
use mixrelay_driver_control::DriverState;
use tauri::menu::{CheckMenuItem, Submenu};

use super::{
    MenuState, UpdateResult,
    choices::{self, SettingAction, SettingChoice},
};
use crate::updates::UpdateState;

/// Text for the supported status menu languages.
#[derive(Clone, Copy)]
pub(super) struct Labels {
    pub(super) loading: &'static str,
    pub(super) check_updates: &'static str,
    pub(super) quit: &'static str,
    pub(super) download: &'static str,
    devices: &'static str,
    channels: &'static str,
    sample_rate: &'static str,
    bit_depth: &'static str,
    mixed: &'static str,
    current: &'static str,
    unpublished: &'static str,
    update_failed: &'static str,
    download_failed: &'static str,
}

const LABELS: [Labels; 11] = [
    Labels {
        loading: "Checking…",
        check_updates: "Check for Updates",
        quit: "Quit",
        download: "Download…",
        devices: "Devices",
        channels: "Channels",
        sample_rate: "Sample Rate",
        bit_depth: "Bit Depth",
        mixed: "Mixed",
        current: "Up to Date",
        unpublished: "No Published Updates",
        update_failed: "Update Check Failed",
        download_failed: "Download Link Failed",
    },
    Labels {
        loading: "확인 중…",
        check_updates: "업데이트 확인",
        quit: "종료",
        download: "다운로드…",
        devices: "장치",
        channels: "채널 수",
        sample_rate: "샘플레이트",
        bit_depth: "비트 깊이",
        mixed: "혼합",
        current: "최신 버전입니다",
        unpublished: "배포된 업데이트 없음",
        update_failed: "업데이트 확인 실패",
        download_failed: "다운로드 링크 열기 실패",
    },
    Labels {
        loading: "確認中…",
        check_updates: "アップデートを確認",
        quit: "終了",
        download: "ダウンロード…",
        devices: "デバイス",
        channels: "チャンネル数",
        sample_rate: "サンプルレート",
        bit_depth: "ビット深度",
        mixed: "混在",
        current: "最新です",
        unpublished: "公開されたアップデートはありません",
        update_failed: "アップデートの確認に失敗",
        download_failed: "ダウンロードリンクを開けませんでした",
    },
    Labels {
        loading: "正在检查…",
        check_updates: "检查更新",
        quit: "退出",
        download: "下载…",
        devices: "设备",
        channels: "声道数",
        sample_rate: "采样率",
        bit_depth: "位深度",
        mixed: "混合",
        current: "已是最新版本",
        unpublished: "没有已发布的更新",
        update_failed: "检查更新失败",
        download_failed: "无法打开下载链接",
    },
    Labels {
        loading: "正在檢查…",
        check_updates: "檢查更新",
        quit: "結束",
        download: "下載…",
        devices: "裝置",
        channels: "聲道數",
        sample_rate: "取樣率",
        bit_depth: "位元深度",
        mixed: "混合",
        current: "已是最新版本",
        unpublished: "沒有已發布的更新",
        update_failed: "檢查更新失敗",
        download_failed: "無法開啟下載連結",
    },
    Labels {
        loading: "Vérification…",
        check_updates: "Rechercher des mises à jour",
        quit: "Quitter",
        download: "Télécharger…",
        devices: "Périphériques",
        channels: "Canaux",
        sample_rate: "Fréquence d’échantillonnage",
        bit_depth: "Profondeur de bits",
        mixed: "Mixte",
        current: "À jour",
        unpublished: "Aucune mise à jour publiée",
        update_failed: "Échec de la vérification des mises à jour",
        download_failed: "Impossible d’ouvrir le lien de téléchargement",
    },
    Labels {
        loading: "Wird geprüft…",
        check_updates: "Nach Updates suchen",
        quit: "Beenden",
        download: "Herunterladen…",
        devices: "Geräte",
        channels: "Kanäle",
        sample_rate: "Abtastrate",
        bit_depth: "Bittiefe",
        mixed: "Gemischt",
        current: "Aktuell",
        unpublished: "Keine veröffentlichten Updates",
        update_failed: "Updateprüfung fehlgeschlagen",
        download_failed: "Downloadlink konnte nicht geöffnet werden",
    },
    Labels {
        loading: "Comprobando…",
        check_updates: "Buscar actualizaciones",
        quit: "Salir",
        download: "Descargar…",
        devices: "Dispositivos",
        channels: "Canales",
        sample_rate: "Frecuencia de muestreo",
        bit_depth: "Profundidad de bits",
        mixed: "Mixto",
        current: "Actualizado",
        unpublished: "No hay actualizaciones publicadas",
        update_failed: "Error al buscar actualizaciones",
        download_failed: "No se pudo abrir el enlace de descarga",
    },
    Labels {
        loading: "A verificar…",
        check_updates: "Procurar atualizações",
        quit: "Sair",
        download: "Baixar…",
        devices: "Dispositivos",
        channels: "Canais",
        sample_rate: "Taxa de amostragem",
        bit_depth: "Profundidade de bits",
        mixed: "Misto",
        current: "Atualizado",
        unpublished: "Não há atualizações publicadas",
        update_failed: "Falha ao verificar atualizações",
        download_failed: "Não foi possível abrir o link de download",
    },
    Labels {
        loading: "Verifica…",
        check_updates: "Verifica aggiornamenti",
        quit: "Esci",
        download: "Scarica…",
        devices: "Dispositivi",
        channels: "Canali",
        sample_rate: "Frequenza di campionamento",
        bit_depth: "Profondità di bit",
        mixed: "Misto",
        current: "Versione aggiornata",
        unpublished: "Nessun aggiornamento pubblicato",
        update_failed: "Verifica aggiornamenti non riuscita",
        download_failed: "Impossibile aprire il link di download",
    },
    Labels {
        loading: "Проверка…",
        check_updates: "Проверить обновления",
        quit: "Выйти",
        download: "Скачать…",
        devices: "Устройства",
        channels: "Каналы",
        sample_rate: "Частота дискретизации",
        bit_depth: "Разрядность",
        mixed: "Смешанный",
        current: "Последняя версия",
        unpublished: "Опубликованных обновлений нет",
        update_failed: "Не удалось проверить обновления",
        download_failed: "Не удалось открыть ссылку загрузки",
    },
];

/// Matches the UI locale, including Chinese region and script aliases.
pub(super) fn labels(locale: &str) -> Labels {
    LABELS[crate::native_text::resolve_locale(locale).index()]
}

/// Displays only the product name and observed installed version.
pub(super) fn status_text(state: &MenuState) -> String {
    state
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.installed_version.as_deref())
        .map_or_else(
            || "MixRelay".to_owned(),
            |version| format!("MixRelay {version}"),
        )
}

/// Uses the same device hierarchy for every observed device count.
pub(super) fn devices_menu(
    app: &tauri::AppHandle,
    state: &mut MenuState,
    text: &Labels,
) -> tauri::Result<Submenu<tauri::Wry>> {
    let devices = device_menus(app, state, text)?;
    let menu = Submenu::new(app, text.devices, !devices.is_empty())?;
    for device in devices {
        menu.append(&device)?;
    }
    Ok(menu)
}

/// Creates named device submenus with sample rate, bit depth, and channel count controls.
fn device_menus(
    app: &tauri::AppHandle,
    state: &mut MenuState,
    text: &Labels,
) -> tauri::Result<Vec<Submenu<tauri::Wry>>> {
    let Some(snapshot) = state.snapshot.clone() else {
        return Ok(Vec::new());
    };
    let mut menus = Vec::with_capacity(snapshot.settings.devices.len());
    for device in &snapshot.settings.devices {
        let submenu = Submenu::new(
            app,
            device.spec.display_name.replace('&', "&&"),
            snapshot.state == DriverState::Ready
                && !state.busy
                && state.audio_change.is_none()
                && !state.stopping,
        )?;
        let mut builder = DeviceMenu {
            app,
            state,
            text,
            device,
            revision: snapshot.revision,
        };
        builder.rates(&submenu)?;
        builder.pcm(&submenu)?;
        builder.channels(
            &submenu,
            &choices::channel_choices(device, &snapshot.settings),
        )?;
        menus.push(submenu);
    }
    Ok(menus)
}

struct DeviceMenu<'a> {
    app: &'a tauri::AppHandle,
    state: &'a mut MenuState,
    text: &'a Labels,
    device: &'a VirtualEndpointDeviceSettings,
    revision: u64,
}

impl DeviceMenu<'_> {
    fn choice(
        &mut self,
        menu: &Submenu<tauri::Wry>,
        name: &str,
        selected: bool,
        choice: SettingChoice,
    ) -> tauri::Result<()> {
        let action = SettingAction {
            endpoint_id: self.device.spec.id.clone(),
            expected_revision: self.revision,
            choice,
        };
        let id = format!(
            "status:choice:{}:{}",
            self.state.generation,
            self.state.actions.len()
        );
        self.state.actions.insert(id.clone(), action);
        menu.append(&CheckMenuItem::with_id(
            self.app,
            id,
            name,
            true,
            selected,
            None::<&str>,
        )?)
    }

    fn channels(&mut self, parent: &Submenu<tauri::Wry>, supported: &[u16]) -> tauri::Result<()> {
        let current = choices::current_channels(self.device);
        let mixed = self.device.spec.render.is_some()
            && self.device.spec.capture.is_some()
            && current.is_none();
        let name = setting_label(
            self.text.channels,
            current.map(|count| count.to_string()),
            mixed,
            self.text,
        );
        let counts = channel_presets(supported);
        let menu = Submenu::new(
            self.app,
            name,
            self.device.observed_sample_rate.is_some() && !counts.is_empty(),
        )?;
        for count in counts {
            self.choice(
                &menu,
                &count.to_string(),
                current == Some(count),
                SettingChoice::Channels(count),
            )?;
        }
        parent.append(&menu)
    }

    fn rates(&mut self, parent: &Submenu<tauri::Wry>) -> tauri::Result<()> {
        let current = self.device.observed_sample_rate;
        let name = setting_label(
            self.text.sample_rate,
            current.map(choices::rate_label),
            false,
            self.text,
        );
        let mut rates = self.device.supported_sample_rates.clone();
        rates.sort_unstable();
        rates.dedup();
        let menu = Submenu::new(
            self.app,
            name,
            self.device.can_set_sample_rate && !rates.is_empty(),
        )?;
        for rate in rates {
            self.choice(
                &menu,
                &choices::rate_label(rate),
                current == Some(rate),
                SettingChoice::SampleRate(rate),
            )?;
        }
        parent.append(&menu)
    }

    fn pcm(&mut self, parent: &Submenu<tauri::Wry>) -> tauri::Result<()> {
        let current = choices::current_sample_format(self.device);
        let mixed = self.device.spec.render.is_some()
            && self.device.spec.capture.is_some()
            && self.device.native_formats.render.current.is_some()
            && self.device.native_formats.capture.current.is_some()
            && current.is_none();
        let name = setting_label(
            self.text.bit_depth,
            current.map(|format| choices::pcm_label(format).to_owned()),
            mixed,
            self.text,
        );
        let options = choices::pcm_choices(self.device);
        let menu = Submenu::new(self.app, name, !options.is_empty())?;
        for format in options {
            self.choice(
                &menu,
                choices::pcm_label(format),
                current == Some(format),
                SettingChoice::SampleFormat(format),
            )?;
        }
        parent.append(&menu)
    }
}

fn channel_presets(supported: &[u16]) -> Vec<u16> {
    [2, 4, 8, 16, 32, 64]
        .into_iter()
        .filter(|count| supported.contains(count))
        .collect()
}

fn setting_label(label: &str, current: Option<String>, mixed: bool, text: &Labels) -> String {
    if mixed {
        return format!("{label} · {}", text.mixed);
    }
    current.map_or_else(|| label.to_owned(), |value| format!("{label} · {value}"))
}

/// Keeps the update action visible and names its active check.
pub(super) fn update_label(busy: bool, text: &Labels) -> &'static str {
    if busy {
        text.loading
    } else {
        text.check_updates
    }
}

/// Selects one result row from validated catalog data; routine automatic results stay quiet.
pub(super) fn update_result(
    state: UpdateState,
    download: Option<(&str, &str)>,
    manual: bool,
) -> Option<UpdateResult> {
    match state {
        UpdateState::Available | UpdateState::Published => download
            .map(|(version, url)| UpdateResult::Download {
                version: version.to_owned(),
                url: url.to_owned(),
            })
            .or_else(|| manual.then_some(UpdateResult::CheckFailed)),
        UpdateState::Current => manual.then_some(UpdateResult::Current),
        UpdateState::NotPublished => manual.then_some(UpdateResult::NotPublished),
        UpdateState::Unavailable => manual.then_some(UpdateResult::CheckFailed),
    }
}

/// Formats one localized result or explicit download action without a dialog.
pub(super) fn update_result_label(result: &UpdateResult, text: &Labels) -> String {
    match result {
        UpdateResult::Current => text.current.to_owned(),
        UpdateResult::NotPublished => text.unpublished.to_owned(),
        UpdateResult::CheckFailed => text.update_failed.to_owned(),
        UpdateResult::Download { version, .. } => format!("{version} · {}", text.download),
        UpdateResult::DownloadFailed => text.download_failed.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_settings_and_update_messages_are_complete_in_supported_languages() {
        for (locale, channels) in [
            ("en", "Channels"),
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
            for message in [
                text.devices,
                text.loading,
                text.check_updates,
                text.quit,
                text.download,
                text.sample_rate,
                text.bit_depth,
                text.mixed,
                text.current,
                text.unpublished,
                text.update_failed,
                text.download_failed,
            ] {
                assert!(!message.is_empty());
                if locale != "en" {
                    assert_ne!(message, "Quit");
                }
            }
        }
        assert_eq!(labels("zh-TW").sample_rate, "取樣率");
        assert_eq!(labels("zh-SG").sample_rate, "采样率");
    }
    use mixrelay_audio_io::VirtualEndpointSettings;
    use mixrelay_driver_control::DriverSnapshot;

    fn header_state(driver_state: DriverState, version: Option<&str>) -> MenuState {
        MenuState {
            snapshot: Some(DriverSnapshot {
                protocol_version: 1,
                revision: 1,
                state: driver_state,
                installed_version: version.map(str::to_owned),
                owner_process_id: None,
                settings: VirtualEndpointSettings {
                    can_create: false,
                    can_rename: false,
                    render_channels: Vec::new(),
                    capture_channels: Vec::new(),
                    creation_template: None,
                    devices: Vec::new(),
                },
                persistence_error: None,
            }),
            ..MenuState::default()
        }
    }

    #[test]
    fn menu_labels_name_device_settings_and_plain_quit_in_both_languages() {
        let en = labels("en");
        let ko = labels("ko-KR");
        assert_eq!((en.quit, ko.quit), ("Quit", "종료"));
        assert_eq!((en.devices, ko.devices), ("Devices", "장치"));
        assert_eq!((en.channels, ko.channels), ("Channels", "채널 수"));
        assert_eq!(
            (en.sample_rate, ko.sample_rate),
            ("Sample Rate", "샘플레이트")
        );
        assert_eq!((en.bit_depth, ko.bit_depth), ("Bit Depth", "비트 깊이"));
        assert_eq!(labels("ja-JP").check_updates, "アップデートを確認");
    }

    #[test]
    fn full_channel_capabilities_offer_only_six_presets() {
        let supported: Vec<_> = (1..=64).collect();
        let counts = channel_presets(&supported);
        assert_eq!(counts, [2, 4, 8, 16, 32, 64]);
    }

    #[test]
    fn sparse_channel_capabilities_offer_only_supported_presets() {
        assert_eq!(channel_presets(&[2, 3, 17, 64]), [2, 64]);
        assert!(channel_presets(&[]).is_empty());
        assert!(channel_presets(&[1, 3, 17]).is_empty());
    }

    #[test]
    fn duplicate_channel_capabilities_produce_one_choice_per_supported_preset() {
        assert_eq!(channel_presets(&[64, 4, 3, 64, 3, 4]), [4, 64]);
    }

    #[test]
    fn nonpreset_current_channels_remain_in_title_without_a_false_check() {
        let supported: Vec<_> = (1..=64).collect();
        let counts = channel_presets(&supported);
        for current in [1, 23, 65] {
            let selected = Some(current);
            assert!(!counts.iter().any(|&count| selected == Some(count)));
            assert_eq!(
                setting_label("Channels", Some(current.to_string()), false, &labels("en")),
                format!("Channels · {current}")
            );
        }
    }

    #[test]
    fn mixed_values_show_a_short_observation_instead_of_a_false_current_value() {
        assert_eq!(
            setting_label("Channels", None, true, &labels("en")),
            "Channels · Mixed"
        );
        assert_eq!(
            setting_label("비트 깊이", None, true, &labels("ko")),
            "비트 깊이 · 혼합"
        );
        assert_eq!(
            setting_label("Channels", Some("2".into()), false, &labels("en")),
            "Channels · 2"
        );
    }

    #[test]
    fn header_only_shows_product_and_observed_version_regardless_of_state() {
        for locale in ["en", "ko-KR"] {
            for driver_state in [
                DriverState::Ready,
                DriverState::NotInstalled,
                DriverState::InstalledNotLoaded,
                DriverState::Unavailable,
            ] {
                for (version, expected) in [
                    (None, "MixRelay"),
                    (Some("0.1.0"), "MixRelay 0.1.0"),
                    (Some("7.3.2"), "MixRelay 7.3.2"),
                ] {
                    let mut state = header_state(driver_state, version);
                    state.locale = locale.into();
                    assert_eq!(status_text(&state), expected);
                    state.busy = true;
                    assert_eq!(status_text(&state), expected);
                    state.busy = false;
                    state.error = Some("raw action failure".into());
                    assert_eq!(status_text(&state), expected);
                    state.error = None;
                    state.snapshot.as_mut().unwrap().persistence_error =
                        Some("raw save failure".into());
                    assert_eq!(status_text(&state), expected);
                }
            }
        }
    }

    #[test]
    fn missing_snapshot_only_shows_product_name() {
        for locale in ["en", "ko-KR"] {
            let mut state = MenuState {
                locale: locale.into(),
                ..MenuState::default()
            };
            assert_eq!(status_text(&state), "MixRelay");
            state.busy = true;
            state.error = Some("raw action failure".into());
            assert_eq!(status_text(&state), "MixRelay");
        }
    }

    #[test]
    fn routine_update_results_are_shown_only_after_manual_checks() {
        for (state, result, english, korean) in [
            (
                UpdateState::Current,
                UpdateResult::Current,
                "Up to Date",
                "최신 버전입니다",
            ),
            (
                UpdateState::NotPublished,
                UpdateResult::NotPublished,
                "No Published Updates",
                "배포된 업데이트 없음",
            ),
            (
                UpdateState::Unavailable,
                UpdateResult::CheckFailed,
                "Update Check Failed",
                "업데이트 확인 실패",
            ),
        ] {
            assert_eq!(update_result(state, None, false), None);
            assert_eq!(update_result(state, None, true), Some(result.clone()));
            assert_eq!(update_result_label(&result, &labels("en")), english);
            assert_eq!(update_result_label(&result, &labels("ko")), korean);
        }
    }

    #[test]
    fn available_downloads_have_one_action_for_manual_and_automatic_checks() {
        let download = Some(("0.2.0", "https://example.com/driver.pkg"));
        let expected = UpdateResult::Download {
            version: "0.2.0".into(),
            url: "https://example.com/driver.pkg".into(),
        };
        for state in [UpdateState::Available, UpdateState::Published] {
            for manual in [false, true] {
                assert_eq!(
                    update_result(state, download, manual),
                    Some(expected.clone())
                );
            }
            assert_eq!(update_result(state, None, false), None);
            assert_eq!(
                update_result(state, None, true),
                Some(UpdateResult::CheckFailed)
            );
        }
        assert_eq!(
            update_result_label(&expected, &labels("en")),
            "0.2.0 · Download…"
        );
        assert_eq!(
            update_result_label(&expected, &labels("ko")),
            "0.2.0 · 다운로드…"
        );
    }

    #[test]
    fn failed_downloads_are_distinct_and_check_updates_remains_available() {
        assert_eq!(
            update_result_label(&UpdateResult::DownloadFailed, &labels("en")),
            "Download Link Failed"
        );
        assert_eq!(
            update_result_label(&UpdateResult::DownloadFailed, &labels("ko")),
            "다운로드 링크 열기 실패"
        );
        assert_eq!(update_label(false, &labels("en")), "Check for Updates");
        assert_eq!(update_label(true, &labels("en")), "Checking…");
        assert_eq!(update_label(false, &labels("ko")), "업데이트 확인");
        assert_eq!(update_label(true, &labels("ko")), "확인 중…");
    }
}
