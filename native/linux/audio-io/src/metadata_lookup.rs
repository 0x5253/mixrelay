//! Desktop-entry and icon-theme precedence over an injected file catalog.
use crate::metadata_cache::{Key, Metadata};
use std::path::{Component, Path, PathBuf};

pub(crate) trait Catalog {
    fn is_file(&self, path: &Path) -> bool;
    fn read_regular(&self, path: &Path) -> Option<Vec<u8>>;
    fn icon_file(&self, path: &Path) -> Option<String>;
}

pub(crate) fn bounded(value: Option<&str>) -> Option<String> {
    value
        .filter(|value| !value.is_empty() && value.len() <= 512)
        .map(str::to_owned)
}

pub(crate) fn theme_from_settings(text: &str) -> Option<String> {
    let mut in_settings = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_settings = line == "[Settings]";
        } else if in_settings && let Some(("gtk-icon-theme-name", value)) = line.split_once('=') {
            return safe_name(value.trim()).then(|| value.trim().to_owned());
        }
    }
    None
}

pub(crate) fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value != "."
        && value != ".."
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
}

fn identifiers(key: &Key) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(id) = &key.application_id {
        names.push(id.trim_end_matches(".desktop").to_owned());
    }
    if let Some(binary) = &key.executable
        && let Some(name) = Path::new(binary).file_name().and_then(|name| name.to_str())
        && !names.iter().any(|id| id == name)
    {
        names.push(name.to_owned());
    }
    names.retain(|name| safe_name(name));
    names
}

pub(crate) fn lookup(
    files: &impl Catalog,
    key: &Key,
    roots: &[PathBuf],
    locale: Option<&str>,
) -> Metadata {
    let names = identifiers(key);
    let desktop = names.iter().find_map(|name| {
        let path = roots
            .iter()
            .map(|root| root.join("applications").join(format!("{name}.desktop")))
            .find(|path| files.is_file(path))?;
        // A user Hidden=true override must not resurrect a system desktop entry.
        let bytes = files.read_regular(&path)?;
        parse_desktop(std::str::from_utf8(&bytes).ok()?, locale)
    });
    let icon_names = key
        .icon_name
        .iter()
        .chain(desktop.as_ref().and_then(|entry| entry.1.as_ref()))
        .chain(names.iter());
    Metadata {
        icon_data_url: icon_names
            .into_iter()
            .find_map(|name| icon(files, name, roots, key.theme_name.as_deref())),
        display_name: desktop.and_then(|entry| entry.0),
    }
}

pub(crate) fn parse_desktop(
    text: &str,
    locale: Option<&str>,
) -> Option<(Option<String>, Option<String>)> {
    let mut in_entry = false;
    let mut name = None;
    let mut localized = None;
    let mut language = None;
    let mut icon = None;
    let mut application = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Type" => application = value == "Application",
            "Hidden" if value == "true" => return None,
            "Name" => name = bounded(Some(value)),
            "Icon" => icon = bounded(Some(value)),
            key if locale.is_some_and(|locale| key == format!("Name[{locale}]")) => {
                localized = bounded(Some(value));
            }
            key if locale.is_some_and(|locale| {
                key == format!("Name[{}]", locale.split('_').next().unwrap_or(locale))
            }) =>
            {
                language = bounded(Some(value));
            }
            _ => {}
        }
    }
    application.then_some((localized.or(language).or(name), icon))
}

fn theme_index(
    files: &impl Catalog,
    theme: &str,
    roots: &[PathBuf],
) -> Option<(Vec<String>, Vec<String>)> {
    let path = roots
        .iter()
        .map(|root| root.join("icons").join(theme).join("index.theme"))
        .find(|path| files.is_file(path))?;
    let bytes = files.read_regular(&path)?;
    let text = std::str::from_utf8(&bytes).ok()?;
    let mut in_theme = false;
    let mut exact = Vec::new();
    let mut scalable = Vec::new();
    let mut other = Vec::new();
    let mut parents = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_theme = line == "[Icon Theme]";
            continue;
        }
        if !in_theme {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Directories" | "ScaledDirectories" => {
                for dir in value.split(',').map(str::trim).filter(|dir| {
                    !dir.is_empty()
                        && dir.len() <= 255
                        && Path::new(dir)
                            .components()
                            .all(|component| matches!(component, Component::Normal(_)))
                }) {
                    let prefix = dir.split('/').next().unwrap_or_default();
                    let category = if prefix.starts_with("32x32") {
                        &mut exact
                    } else if prefix == "scalable" {
                        &mut scalable
                    } else {
                        &mut other
                    };
                    if category.len() < 128 {
                        category.push(dir.to_owned());
                    }
                }
            }
            "Inherits" => {
                let remaining = 16usize.saturating_sub(parents.len());
                parents.extend(
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|parent| safe_name(parent))
                        .take(remaining)
                        .map(str::to_owned),
                );
            }
            _ => {}
        }
    }
    // Real hicolor indexes list many sizes before scalable. Keep the lookup
    // bounded without dropping the preferred SVG directory near the end.
    exact.extend(scalable);
    exact.extend(other);
    exact.truncate(128);
    Some((exact, parents))
}

fn icon_in_theme(
    files: &impl Catalog,
    theme: &str,
    name: &str,
    roots: &[PathBuf],
    visited: &mut Vec<String>,
) -> Option<String> {
    if !safe_name(theme) || visited.len() == 16 || visited.iter().any(|seen| seen == theme) {
        return None;
    }
    visited.push(theme.to_owned());
    let (directories, parents) = theme_index(files, theme, roots)?;
    // A 32 px image is preferred; scalable SVG is next. A selected theme's
    // other sizes still win over inherited themes, as the XDG contract requires.
    for dir in directories {
        for root in roots {
            for extension in ["png", "svg"] {
                let path = root
                    .join("icons")
                    .join(theme)
                    .join(&dir)
                    .join(format!("{name}.{extension}"));
                if let Some(image) = files.icon_file(&path) {
                    return Some(image);
                }
            }
        }
    }
    parents
        .into_iter()
        .find_map(|parent| icon_in_theme(files, &parent, name, roots, visited))
}

pub(crate) fn icon(
    files: &impl Catalog,
    name: &str,
    roots: &[PathBuf],
    selected_theme: Option<&str>,
) -> Option<String> {
    let path = Path::new(name);
    if path.is_absolute() {
        return files.icon_file(path);
    }
    if !safe_name(name) {
        return None;
    }
    let name = name
        .strip_suffix(".png")
        .or_else(|| name.strip_suffix(".svg"))
        .unwrap_or(name);
    selected_theme
        .and_then(|theme| icon_in_theme(files, theme, name, roots, &mut Vec::new()))
        .or_else(|| icon_in_theme(files, "Adwaita", name, roots, &mut Vec::new()))
        .or_else(|| icon_in_theme(files, "hicolor", name, roots, &mut Vec::new()))
        .or_else(|| {
            roots.iter().find_map(|root| {
                [root.join("pixmaps"), root.join("icons")]
                    .into_iter()
                    .find_map(|dir| {
                        ["png", "svg"].into_iter().find_map(|extension| {
                            files.icon_file(&dir.join(format!("{name}.{extension}")))
                        })
                    })
            })
        })
}

#[cfg(test)]
mod tests;
