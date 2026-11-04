//! Renders a status menu model into the native tray menu.
use std::collections::HashMap;

use tauri::{
    AppHandle, Wry,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
};

use super::model::{DevicesMenu, Entry, ItemId, SettingMenu, StatusMenuModel};

/// Native items for the fixed rows, created once so their IDs and shortcuts persist.
pub(super) struct FixedItems(HashMap<ItemId, MenuItem<Wry>>);

impl FixedItems {
    /// Creates every fixed row blank; the first render sets its text and enablement.
    pub(super) fn new(app: &AppHandle) -> tauri::Result<Self> {
        ItemId::ALL
            .into_iter()
            .map(|id| {
                MenuItem::with_id(app, id.id(), "", false, id.accelerator()).map(|item| (id, item))
            })
            .collect::<tauri::Result<_>>()
            .map(Self)
    }
}

/// Rebuilds the tray menu from `model`.
pub(super) fn render(
    app: &AppHandle,
    tray: &Menu<Wry>,
    items: &FixedItems,
    model: &StatusMenuModel,
) -> tauri::Result<()> {
    for item in tray.items()? {
        tray.remove(&item)?;
    }
    for entry in &model.entries {
        match entry {
            Entry::Item(item) => {
                let native = &items.0[&item.id];
                native.set_text(&item.label)?;
                native.set_enabled(item.enabled)?;
                tray.append(native)?;
            }
            Entry::Devices(devices) => tray.append(&devices_submenu(app, devices)?)?,
            Entry::Separator => tray.append(&PredefinedMenuItem::separator(app)?)?,
        }
    }
    Ok(())
}

fn devices_submenu(app: &AppHandle, devices: &DevicesMenu) -> tauri::Result<Submenu<Wry>> {
    let menu = Submenu::new(app, devices.label, !devices.devices.is_empty())?;
    for device in &devices.devices {
        let submenu = Submenu::new(app, &device.name, device.enabled)?;
        for setting in &device.settings {
            submenu.append(&setting_submenu(app, setting)?)?;
        }
        menu.append(&submenu)?;
    }
    Ok(menu)
}

fn setting_submenu(app: &AppHandle, setting: &SettingMenu) -> tauri::Result<Submenu<Wry>> {
    let menu = Submenu::new(app, &setting.label, setting.enabled)?;
    for choice in &setting.choices {
        menu.append(&CheckMenuItem::with_id(
            app,
            &choice.id,
            &choice.label,
            true,
            choice.checked,
            None::<&str>,
        )?)?;
    }
    Ok(menu)
}
