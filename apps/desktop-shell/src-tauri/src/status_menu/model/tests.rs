use mixrelay_audio_io::{
    ChannelLayout, NativeDeviceFormats, NativeSampleFormat, NativeStreamFormat,
    NativeStreamFormats, StreamSpec, VirtualEndpointId, VirtualEndpointSpec,
};
use mixrelay_control_protocol::EnginePowerStatus;
use mixrelay_driver_core::{DriverSnapshot, DriverState};

use super::*;

const INSTALLED: Environment = Environment {
    editor_installed: true,
    diagnostics_exporting: false,
};

fn state(locale: &str) -> MenuState {
    MenuState {
        locale: locale.into(),
        ..MenuState::default()
    }
}

fn items(model: &StatusMenuModel) -> Vec<(ItemId, &str, bool)> {
    model
        .entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Item(item) => Some((item.id, item.label.as_str(), item.enabled)),
            _ => None,
        })
        .collect()
}

fn item(model: &StatusMenuModel, id: ItemId) -> Option<(&str, bool)> {
    items(model)
        .into_iter()
        .find(|(item, ..)| *item == id)
        .map(|(_, label, enabled)| (label, enabled))
}

fn devices(model: &StatusMenuModel) -> &DevicesMenu {
    model
        .entries
        .iter()
        .find_map(|entry| match entry {
            Entry::Devices(devices) => Some(devices),
            _ => None,
        })
        .expect("the Devices submenu is always present")
}

fn power(enabled: bool, transitioning: bool) -> EnginePowerStatus {
    EnginePowerStatus {
        enabled,
        transitioning,
        unsaved: false,
        pause_reason: None,
    }
}

fn format(channels: u16, sample_format: NativeSampleFormat) -> NativeStreamFormat {
    NativeStreamFormat {
        sample_rate: 48_000,
        channels,
        sample_format,
    }
}

/// A two-way device at 48 kHz float with stereo playback and four-channel capture.
fn device() -> VirtualEndpointDeviceSettings {
    let render = format(2, NativeSampleFormat::Float32);
    let capture = format(4, NativeSampleFormat::Float32);
    VirtualEndpointDeviceSettings {
        spec: VirtualEndpointSpec {
            id: VirtualEndpointId::from("device"),
            display_name: "Mic & Music".into(),
            render: Some(StreamSpec {
                channels: 2,
                channel_layout: ChannelLayout::Stereo,
            }),
            capture: Some(StreamSpec {
                channels: 4,
                channel_layout: ChannelLayout::Discrete,
            }),
            supported_sample_rates: vec![44_100, 48_000, 96_000],
            default_sample_rate: 48_000,
        },
        native_device_ids: vec!["native".into()],
        observed_sample_rate: Some(48_000),
        supported_sample_rates: vec![96_000, 48_000, 44_100, 48_000],
        can_set_sample_rate: true,
        native_formats: NativeDeviceFormats {
            render: NativeStreamFormats {
                current: Some(render),
                supported: vec![render, format(2, NativeSampleFormat::Signed24)],
            },
            capture: NativeStreamFormats {
                current: Some(capture),
                supported: vec![capture, format(4, NativeSampleFormat::Signed24)],
            },
        },
    }
}

fn snapshot(driver: DriverState, devices: Vec<VirtualEndpointDeviceSettings>) -> DriverSnapshot {
    DriverSnapshot {
        protocol_version: 1,
        revision: 7,
        state: driver,
        installed_version: None,
        owner_process_id: None,
        settings: VirtualEndpointSettings {
            can_create: false,
            can_rename: false,
            render_channels: (1..=64).collect(),
            capture_channels: vec![2, 4, 8, 23],
            creation_template: None,
            devices,
        },
        persistence_error: None,
    }
}

fn ready_state() -> MenuState {
    MenuState {
        snapshot: Some(snapshot(DriverState::Ready, vec![device()])),
        generation: 3,
        ..state("en")
    }
}

fn labels_of(setting: &SettingMenu) -> Vec<(&str, bool)> {
    setting
        .choices
        .iter()
        .map(|choice| (choice.label.as_str(), choice.checked))
        .collect()
}

#[test]
fn a_fresh_menu_lists_every_row_in_order_with_only_safe_actions_enabled() {
    let model = StatusMenuModel::build(&state("en"), INSTALLED);
    assert!(matches!(model.entries[1], Entry::Devices(_)));
    assert!(matches!(model.entries[4], Entry::Separator));
    assert_eq!(
        items(&model),
        [
            (ItemId::Status, "MixRelay", false),
            (ItemId::OpenEditor, "Open MixRelay", true),
            (ItemId::Pause, "Pause", false),
            (ItemId::CheckUpdates, "Check for Updates", true),
            (ItemId::Diagnostics, "Save Diagnostic Report…", true),
            (ItemId::Quit, "Quit", true),
        ]
    );
    let devices = devices(&model);
    assert_eq!(devices.label, "Devices");
    assert!(devices.devices.is_empty());
}

#[test]
fn the_whole_menu_follows_the_selected_language() {
    let mut korean = state("ko-KR");
    korean.update_result = Some(UpdateResult::Current);
    let model = StatusMenuModel::build(&korean, INSTALLED);
    assert_eq!(
        items(&model),
        [
            (ItemId::Status, "MixRelay", false),
            (ItemId::OpenEditor, "MixRelay 열기", true),
            (ItemId::Pause, "일시정지", false),
            (ItemId::CheckUpdates, "업데이트 확인", true),
            (ItemId::Download, "최신 버전입니다", false),
            (ItemId::Diagnostics, "진단 리포트 저장…", true),
            (ItemId::Quit, "종료", true),
        ]
    );
    assert_eq!(devices(&model).label, "장치");
}

#[test]
fn open_editor_appears_only_when_the_editor_is_installed() {
    let model = StatusMenuModel::build(
        &state("en"),
        Environment {
            editor_installed: false,
            ..INSTALLED
        },
    );
    assert_eq!(item(&model, ItemId::OpenEditor), None);
}

#[test]
fn diagnostics_is_disabled_while_a_report_is_being_saved() {
    let model = StatusMenuModel::build(
        &state("en"),
        Environment {
            diagnostics_exporting: true,
            ..INSTALLED
        },
    );
    assert_eq!(
        item(&model, ItemId::Diagnostics),
        Some(("Save Diagnostic Report…", false))
    );
}

#[test]
fn the_header_shows_only_the_product_and_observed_version() {
    let header = |menu: &MenuState| {
        item(&StatusMenuModel::build(menu, INSTALLED), ItemId::Status)
            .map(|(label, enabled)| (label.to_owned(), enabled))
    };
    assert_eq!(header(&state("en")), Some(("MixRelay".into(), false)));
    for driver in [
        DriverState::Ready,
        DriverState::NotInstalled,
        DriverState::Unavailable,
    ] {
        let mut menu = state("ko-KR");
        menu.snapshot = Some(snapshot(driver, Vec::new()));
        menu.busy = true;
        menu.error = Some("raw action failure".into());
        menu.snapshot.as_mut().unwrap().persistence_error = Some("raw save failure".into());
        assert_eq!(header(&menu), Some(("MixRelay".into(), false)));
        menu.snapshot.as_mut().unwrap().installed_version = Some("7.3.2".into());
        assert_eq!(header(&menu), Some(("MixRelay 7.3.2".into(), false)));
    }
}

#[test]
fn pause_names_the_next_action_and_is_enabled_only_when_a_toggle_can_start() {
    let mut menu = state("en");
    menu.audio_power = Some(power(true, false));
    let pause = |menu: &MenuState| {
        let model = StatusMenuModel::build(menu, INSTALLED);
        item(&model, ItemId::Pause).map(|(label, enabled)| (label.to_owned(), enabled))
    };
    assert_eq!(pause(&menu), Some(("Pause".into(), true)));
    menu.audio_power = Some(power(false, false));
    assert_eq!(pause(&menu), Some(("Resume".into(), true)));
    menu.audio_power = Some(power(false, true));
    assert_eq!(pause(&menu), Some(("Changing audio state…".into(), false)));
    menu.audio_power = Some(power(true, false));
    menu.audio_change = Some(false);
    assert_eq!(pause(&menu), Some(("Changing audio state…".into(), false)));
    menu.audio_change = None;
    menu.busy = true;
    assert_eq!(pause(&menu), Some(("Pause".into(), false)));
}

#[test]
fn quitting_disables_quit_pause_and_device_settings() {
    let mut menu = ready_state();
    menu.audio_power = Some(power(true, false));
    menu.stopping = true;
    let model = StatusMenuModel::build(&menu, INSTALLED);
    assert_eq!(item(&model, ItemId::Quit), Some(("Quit", false)));
    assert_eq!(item(&model, ItemId::Pause), Some(("Pause", false)));
    assert!(!devices(&model).devices[0].enabled);
}

#[test]
fn update_checks_show_progress_and_offer_a_download_only_when_one_exists() {
    let mut menu = state("en");
    menu.updates_busy = true;
    let model = StatusMenuModel::build(&menu, INSTALLED);
    assert_eq!(
        item(&model, ItemId::CheckUpdates),
        Some(("Checking…", false))
    );
    assert_eq!(item(&model, ItemId::Download), None);

    menu.update_result = Some(UpdateResult::Download {
        version: "0.2.0".into(),
        url: "https://example.com/a.pkg".into(),
    });
    let model = StatusMenuModel::build(&menu, INSTALLED);
    assert_eq!(
        item(&model, ItemId::Download),
        Some(("0.2.0 · Download…", false)),
        "no second request while one runs"
    );
    menu.updates_busy = false;
    let model = StatusMenuModel::build(&menu, INSTALLED);
    assert_eq!(
        item(&model, ItemId::Download),
        Some(("0.2.0 · Download…", true))
    );
    assert_eq!(
        item(&model, ItemId::CheckUpdates),
        Some(("Check for Updates", true))
    );

    for (result, label) in [
        (UpdateResult::Current, "Up to Date"),
        (UpdateResult::NotPublished, "No Published Updates"),
        (UpdateResult::CheckFailed, "Update Check Failed"),
        (UpdateResult::DownloadFailed, "Download Link Failed"),
    ] {
        menu.update_result = Some(result);
        let model = StatusMenuModel::build(&menu, INSTALLED);
        assert_eq!(item(&model, ItemId::Download), Some((label, false)));
    }
}

#[test]
fn each_device_offers_supported_rates_bit_depths_and_channel_presets() {
    let model = StatusMenuModel::build(&ready_state(), INSTALLED);
    let [device] = devices(&model).devices.as_slice() else {
        panic!("one device");
    };
    assert_eq!(device.name, "Mic && Music");
    assert!(device.enabled);
    let [rate, depth, channels] = &device.settings;
    assert_eq!(rate.label, "Sample Rate · 48 kHz");
    assert!(rate.enabled);
    assert_eq!(
        labels_of(rate),
        [("44.1 kHz", false), ("48 kHz", true), ("96 kHz", false)]
    );
    assert_eq!(depth.label, "Bit Depth · 32 bit (float)");
    assert_eq!(
        labels_of(depth),
        [("24 bit", false), ("32 bit (float)", true)]
    );
    // Playback and capture disagree, so no channel count is current.
    assert_eq!(channels.label, "Channels · Mixed");
    assert_eq!(
        labels_of(channels),
        [("2", false), ("4", false), ("8", false)]
    );
}

#[test]
fn every_choice_has_a_unique_id_for_this_render_and_carries_the_observed_revision() {
    let model = StatusMenuModel::build(&ready_state(), INSTALLED);
    let actions = model.actions();
    let choices: Vec<_> = devices(&model).devices[0]
        .settings
        .iter()
        .flat_map(|setting| &setting.choices)
        .collect();
    assert_eq!(actions.len(), choices.len());
    for (index, choice) in choices.iter().enumerate() {
        assert_eq!(choice.id, format!("status:choice:3:{index}"));
        assert_eq!(actions[&choice.id], choice.action);
        assert_eq!(choice.action.endpoint_id, VirtualEndpointId::from("device"));
        assert_eq!(choice.action.expected_revision, 7);
    }
    assert_eq!(choices[0].action.choice, SettingChoice::SampleRate(44_100));
    assert_eq!(
        choices.last().unwrap().action.choice,
        SettingChoice::Channels(8)
    );
}

#[test]
fn device_settings_are_locked_unless_a_ready_driver_is_idle() {
    let mut menu = ready_state();
    menu.snapshot.as_mut().unwrap().state = DriverState::InstalledNotLoaded;
    assert!(!devices(&StatusMenuModel::build(&menu, INSTALLED)).devices[0].enabled);
    let changes: [fn(&mut MenuState); 2] = [
        |menu| menu.busy = true,
        |menu| menu.audio_change = Some(true),
    ];
    for change in changes {
        let mut menu = ready_state();
        change(&mut menu);
        assert!(!devices(&StatusMenuModel::build(&menu, INSTALLED)).devices[0].enabled);
    }
}

#[test]
fn settings_without_supported_values_are_disabled_but_still_titled() {
    let mut device = device();
    device.can_set_sample_rate = false;
    device.observed_sample_rate = None;
    device.spec.capture = None;
    device.native_formats.render.current = None;
    let menu = MenuState {
        snapshot: Some(snapshot(DriverState::Ready, vec![device])),
        ..state("en")
    };
    let model = StatusMenuModel::build(&menu, INSTALLED);
    let [rate, depth, channels] = &devices(&model).devices[0].settings;
    assert_eq!((rate.label.as_str(), rate.enabled), ("Sample Rate", false));
    assert_eq!((depth.label.as_str(), depth.enabled), ("Bit Depth", false));
    assert_eq!(
        (channels.label.as_str(), channels.enabled),
        ("Channels · 2", false)
    );
}

#[test]
fn full_channel_capabilities_offer_only_six_presets() {
    let supported: Vec<_> = (1..=64).collect();
    assert_eq!(channel_presets(&supported), [2, 4, 8, 16, 32, 64]);
}

#[test]
fn sparse_channel_capabilities_offer_only_supported_presets() {
    assert_eq!(channel_presets(&[2, 3, 17, 64]), [2, 64]);
    assert!(channel_presets(&[]).is_empty());
    assert!(channel_presets(&[1, 3, 17]).is_empty());
    assert_eq!(channel_presets(&[64, 4, 3, 64, 3, 4]), [4, 64]);
}

#[test]
fn an_unusual_current_value_stays_in_the_title_and_mixed_values_say_so() {
    let en = labels("en");
    assert_eq!(
        setting_label("Channels", Some("23".into()), false, &en),
        "Channels · 23"
    );
    assert_eq!(
        setting_label("Channels", None, true, &en),
        "Channels · Mixed"
    );
    assert_eq!(
        setting_label("비트 깊이", None, true, &labels("ko")),
        "비트 깊이 · 혼합"
    );
}

#[test]
fn every_fixed_row_reports_one_event_id_and_only_quit_has_a_shortcut() {
    for item in ItemId::ALL {
        assert_eq!(ItemId::parse(item.id()), Some(item));
        assert_eq!(item.accelerator().is_some(), item == ItemId::Quit);
    }
    assert_eq!(ItemId::parse("status:choice:3:0"), None);
}
