//! The status menu's rows, labels and enablement, computed from its state.
use std::collections::HashMap;

use mixrelay_audio_io::{VirtualEndpointDeviceSettings, VirtualEndpointSettings};
use mixrelay_desktop_core::devices::{
    SettingAction, SettingChoice, channel_choices, current_channels, current_sample_format,
    pcm_choices, pcm_label, rate_label,
};

use super::{
    labels::{Labels, labels},
    state::{MenuState, UpdateResult},
};

/// A fixed status menu row and the event ID it reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ItemId {
    Status,
    OpenEditor,
    Pause,
    CheckUpdates,
    Download,
    Diagnostics,
    Quit,
}

impl ItemId {
    pub(super) const ALL: [Self; 7] = [
        Self::Status,
        Self::OpenEditor,
        Self::Pause,
        Self::CheckUpdates,
        Self::Download,
        Self::Diagnostics,
        Self::Quit,
    ];

    pub(super) const fn id(self) -> &'static str {
        match self {
            Self::Status => "status:status",
            Self::OpenEditor => "status:open-editor",
            Self::Pause => "status:pause",
            Self::CheckUpdates => "status:updates",
            Self::Download => "status:download",
            Self::Diagnostics => "status:diagnostics",
            Self::Quit => "status:quit",
        }
    }

    pub(super) fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|item| item.id() == id)
    }

    pub(super) const fn accelerator(self) -> Option<&'static str> {
        match self {
            Self::Quit => Some("CmdOrCtrl+Q"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Item {
    pub(super) id: ItemId,
    pub(super) label: String,
    pub(super) enabled: bool,
}

#[derive(Debug, PartialEq)]
pub(super) enum Entry {
    Item(Item),
    Devices(DevicesMenu),
    Separator,
}

/// Devices → one submenu per virtual device.
#[derive(Debug, PartialEq)]
pub(super) struct DevicesMenu {
    pub(super) label: &'static str,
    pub(super) devices: Vec<DeviceMenu>,
}

#[derive(Debug, PartialEq)]
pub(super) struct DeviceMenu {
    pub(super) name: String,
    pub(super) enabled: bool,
    /// Sample rate, bit depth and channels, in that order.
    pub(super) settings: [SettingMenu; 3],
}

/// A setting titled with its current value, offering the supported values as checks.
#[derive(Debug, PartialEq)]
pub(super) struct SettingMenu {
    pub(super) label: String,
    pub(super) enabled: bool,
    pub(super) choices: Vec<Choice>,
}

#[derive(Debug, PartialEq)]
pub(super) struct Choice {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) checked: bool,
    pub(super) action: SettingAction,
}

/// Facts about the app outside the menu's own state.
#[derive(Clone, Copy, Debug)]
pub(super) struct Environment {
    pub(super) editor_installed: bool,
    pub(super) diagnostics_exporting: bool,
}

#[derive(Debug, PartialEq)]
pub(super) struct StatusMenuModel {
    pub(super) entries: Vec<Entry>,
}

impl StatusMenuModel {
    pub(super) fn build(state: &MenuState, environment: Environment) -> Self {
        let text = labels(&state.locale);
        let item = |id, label: &str, enabled| {
            Entry::Item(Item {
                id,
                label: label.to_owned(),
                enabled,
            })
        };
        let mut entries = vec![
            item(ItemId::Status, &status_text(state), false),
            Entry::Devices(devices_menu(state, &text)),
        ];
        if environment.editor_installed {
            entries.push(item(ItemId::OpenEditor, text.open_editor, true));
        }
        entries.extend([
            item(
                ItemId::Pause,
                pause_label(state, &text),
                state.can_toggle_audio(),
            ),
            Entry::Separator,
            item(
                ItemId::CheckUpdates,
                update_label(state.updates_busy, &text),
                !state.updates_busy,
            ),
        ]);
        if let Some(result) = &state.update_result {
            let offers_download = matches!(result, UpdateResult::Download { .. });
            entries.push(item(
                ItemId::Download,
                &update_result_label(result, &text),
                offers_download && !state.updates_busy,
            ));
        }
        entries.extend([
            item(
                ItemId::Diagnostics,
                crate::diagnostics::label(&state.locale),
                !environment.diagnostics_exporting,
            ),
            item(ItemId::Quit, text.quit, !state.stopping),
        ]);
        Self { entries }
    }

    /// The setting change behind each choice ID in this menu.
    pub(super) fn actions(&self) -> HashMap<String, SettingAction> {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Devices(devices) => Some(devices),
                _ => None,
            })
            .flat_map(|devices| &devices.devices)
            .flat_map(|device| &device.settings)
            .flat_map(|setting| &setting.choices)
            .map(|choice| (choice.id.clone(), choice.action.clone()))
            .collect()
    }
}

/// Displays only the product name and observed installed version.
fn status_text(state: &MenuState) -> String {
    state
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.installed_version.as_deref())
        .map_or_else(
            || "MixRelay".to_owned(),
            |version| format!("MixRelay {version}"),
        )
}

fn pause_label(state: &MenuState, text: &Labels) -> &'static str {
    if state.audio_pending() {
        text.changing_audio
    } else if state.audio_power.is_some_and(|power| !power.enabled) {
        text.resume
    } else {
        text.pause
    }
}

/// Keeps the update action visible and names its active check.
fn update_label(busy: bool, text: &Labels) -> &'static str {
    if busy {
        text.checking
    } else {
        text.check_updates
    }
}

/// Formats one localized result or explicit download action without a dialog.
fn update_result_label(result: &UpdateResult, text: &Labels) -> String {
    match result {
        UpdateResult::Current => text.current.to_owned(),
        UpdateResult::NotPublished => text.unpublished.to_owned(),
        UpdateResult::CheckFailed => text.update_failed.to_owned(),
        UpdateResult::Download { version, .. } => format!("{version} · {}", text.download),
        UpdateResult::DownloadFailed => text.download_failed.to_owned(),
    }
}

/// Uses the same device hierarchy for every observed device count.
fn devices_menu(state: &MenuState, text: &Labels) -> DevicesMenu {
    let mut choices = ChoiceIds {
        generation: state.generation,
        next: 0,
    };
    let devices = state.snapshot.as_ref().map_or_else(Vec::new, |snapshot| {
        snapshot
            .settings
            .devices
            .iter()
            .map(|device| {
                let mut settings = DeviceSettings {
                    text,
                    device,
                    revision: snapshot.revision,
                    choices: &mut choices,
                };
                DeviceMenu {
                    // A single `&` would mark a mnemonic in native menus.
                    name: device.spec.display_name.replace('&', "&&"),
                    enabled: state.devices_editable(),
                    settings: [
                        settings.sample_rate(),
                        settings.bit_depth(),
                        settings.channels(&snapshot.settings),
                    ],
                }
            })
            .collect()
    });
    DevicesMenu {
        label: text.devices,
        devices,
    }
}

/// Numbers choices in menu order; the render generation keeps IDs from stale menus inert.
struct ChoiceIds {
    generation: u64,
    next: usize,
}

impl ChoiceIds {
    fn next(&mut self) -> String {
        let id = format!("status:choice:{}:{}", self.generation, self.next);
        self.next += 1;
        id
    }
}

struct DeviceSettings<'a> {
    text: &'a Labels,
    device: &'a VirtualEndpointDeviceSettings,
    revision: u64,
    choices: &'a mut ChoiceIds,
}

impl DeviceSettings<'_> {
    fn choice(&mut self, label: String, checked: bool, choice: SettingChoice) -> Choice {
        Choice {
            id: self.choices.next(),
            label,
            checked,
            action: SettingAction {
                endpoint_id: self.device.spec.id.clone(),
                expected_revision: self.revision,
                choice,
            },
        }
    }

    /// With both render and capture, their values can disagree and no single one is current.
    fn has_both_directions(&self) -> bool {
        self.device.spec.render.is_some() && self.device.spec.capture.is_some()
    }

    fn sample_rate(&mut self) -> SettingMenu {
        let current = self.device.observed_sample_rate;
        let mut rates = self.device.supported_sample_rates.clone();
        rates.sort_unstable();
        rates.dedup();
        SettingMenu {
            label: setting_label(
                self.text.sample_rate,
                current.map(rate_label),
                false,
                self.text,
            ),
            enabled: self.device.can_set_sample_rate && !rates.is_empty(),
            choices: rates
                .into_iter()
                .map(|rate| {
                    self.choice(
                        rate_label(rate),
                        current == Some(rate),
                        SettingChoice::SampleRate(rate),
                    )
                })
                .collect(),
        }
    }

    fn bit_depth(&mut self) -> SettingMenu {
        let current = current_sample_format(self.device);
        let formats = &self.device.native_formats;
        let mixed = self.has_both_directions()
            && formats.render.current.is_some()
            && formats.capture.current.is_some()
            && current.is_none();
        let options = pcm_choices(self.device);
        SettingMenu {
            label: setting_label(
                self.text.bit_depth,
                current.map(|format| pcm_label(format).to_owned()),
                mixed,
                self.text,
            ),
            enabled: !options.is_empty(),
            choices: options
                .into_iter()
                .map(|format| {
                    self.choice(
                        pcm_label(format).to_owned(),
                        current == Some(format),
                        SettingChoice::SampleFormat(format),
                    )
                })
                .collect(),
        }
    }

    fn channels(&mut self, settings: &VirtualEndpointSettings) -> SettingMenu {
        let current = current_channels(self.device);
        let mixed = self.has_both_directions() && current.is_none();
        let counts = channel_presets(&channel_choices(self.device, settings));
        SettingMenu {
            label: setting_label(
                self.text.channels,
                current.map(|count| count.to_string()),
                mixed,
                self.text,
            ),
            enabled: self.device.observed_sample_rate.is_some() && !counts.is_empty(),
            choices: counts
                .into_iter()
                .map(|count| {
                    self.choice(
                        count.to_string(),
                        current == Some(count),
                        SettingChoice::Channels(count),
                    )
                })
                .collect(),
        }
    }
}

/// Only common layouts are offered; an unusual current count still shows in the title.
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

#[cfg(test)]
mod tests;
