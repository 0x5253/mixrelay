//! Bounded, passive desktop metadata lookup. Never execute a desktop entry's
//! `Exec`/`TryExec`, invoke a shell, or expose a filesystem URL to the webview.
//! XDG theme lookup rasterizes bounded SVG icons to PNG for the webview. Missing
//! themes and unsupported images leave the UI's existing fallback intact.

use std::{
    env,
    fs::OpenOptions,
    io::{Cursor, Read},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use gdk_pixbuf::{PixbufLoader, prelude::PixbufLoaderExt};

use super::PipeWireTarget;

const MAX_FILE_BYTES: usize = 64 * 1024;
const ICON_SIZE: i32 = 32;

use crate::metadata_cache::{Environment, MetadataEnvironment, MetadataFiles, Resolver};
pub(super) use crate::metadata_cache::{Key, Metadata};

pub(super) struct DesktopEnvironment;
pub(super) struct DesktopFiles;

impl MetadataEnvironment for DesktopEnvironment {
    fn snapshot(&self) -> Environment {
        Environment {
            roots: data_roots(),
            locale: locale(),
            theme_name: selected_theme(),
        }
    }
}
impl MetadataFiles for DesktopFiles {
    fn lookup(&self, key: &Key, roots: &[PathBuf], locale: Option<&str>) -> Metadata {
        crate::metadata_lookup::lookup(self, key, roots, locale)
    }
}
impl crate::metadata_lookup::Catalog for DesktopFiles {
    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }
    fn read_regular(&self, path: &Path) -> Option<Vec<u8>> {
        read_regular(path)
    }
    fn icon_file(&self, path: &Path) -> Option<String> {
        icon_file(path)
    }
}

pub(super) fn resolve<
    C: mixrelay_time::MonotonicClock,
    E: MetadataEnvironment,
    F: MetadataFiles,
>(
    resolver: &Resolver<C, E, F>,
    target: &PipeWireTarget,
) -> Metadata {
    resolver.resolve(Key {
        application_id: crate::metadata_lookup::bounded(target.application_id.as_deref()),
        executable: crate::metadata_lookup::bounded(target.executable.as_deref()),
        icon_name: crate::metadata_lookup::bounded(target.icon_name.as_deref()),
        theme_name: None,
    })
}

fn data_roots() -> Vec<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let user_data = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.as_ref().map(|path| path.join(".local/share")));
    let mut roots = user_data.into_iter().collect::<Vec<_>>();
    let dirs = env::var_os("XDG_DATA_DIRS")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    roots.extend(
        env::split_paths(&dirs)
            .filter(|path| path.is_absolute())
            .take(5),
    );
    if let Some(home) = home {
        roots.push(home.join(".local/share/flatpak/exports/share"));
    }
    roots.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    roots
}

fn locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|key| env::var(key).ok().filter(|value| !value.is_empty()))
        .and_then(|value| crate::metadata_lookup::bounded(value.split('.').next()))
}

fn selected_theme() -> Option<String> {
    let config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    let bytes = read_regular(&config.join("gtk-3.0/settings.ini"))?;
    let text = std::str::from_utf8(&bytes).ok()?;
    crate::metadata_lookup::theme_from_settings(text)
}

fn read_regular(path: &Path) -> Option<Vec<u8>> {
    // Check before open so FIFOs/devices from a bad entry cannot block discovery.
    // User-owned files may be symlinks (normal for Flatpak exports).
    let metadata = path.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES as u64 {
        return None;
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_FILE_BYTES).then_some(bytes)
}

fn png(path: &Path) -> Option<String> {
    let bytes = read_regular(path)?;
    // Restrict the image contract to raster PNG. SVG/XPM are not embedded as
    // unchecked markup or converted by launching desktop commands.
    let valid_header = bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR");
    let dimensions = valid_header
        && [16, 20].into_iter().all(|offset| {
            u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap_or_default()) <= 1024
                && bytes[offset..offset + 4] != [0; 4]
        });
    if !dimensions {
        return None;
    }
    let mut decoder = png::Decoder::new(Cursor::new(&bytes));
    decoder.set_limits(png::Limits {
        bytes: 4 * 1024 * 1024,
    });
    let mut reader = decoder.read_info().ok()?;
    let output_size = reader.output_buffer_size();
    if output_size > 4 * 1024 * 1024 {
        return None;
    }
    reader.next_frame(&mut vec![0; output_size]).ok()?;
    Some(format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
}

fn svg(path: &Path) -> Option<String> {
    // Feed bytes without a base URI. Librsvg cannot resolve file/network
    // references from an icon supplied by another application.
    let bytes = read_regular(path)?;
    let loader = PixbufLoader::with_type("svg").ok()?;
    loader.set_size(ICON_SIZE, ICON_SIZE);
    let written = loader.write(&bytes);
    let closed = loader.close();
    written.ok()?;
    closed.ok()?;
    let pixbuf = loader.pixbuf()?;
    if !(1..=ICON_SIZE).contains(&pixbuf.width()) || !(1..=ICON_SIZE).contains(&pixbuf.height()) {
        return None;
    }
    let png = pixbuf.save_to_bufferv("png", &[]).ok()?;
    (png.len() <= MAX_FILE_BYTES).then(|| format!("data:image/png;base64,{}", STANDARD.encode(png)))
}

fn icon_file(path: &Path) -> Option<String> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("svg") => svg(path),
        // Preserve absolute PNG Icon paths without an extension.
        _ => png(path),
    }
}

#[cfg(test)]
fn lookup(key: &Key, roots: &[PathBuf], locale: Option<&str>) -> Metadata {
    crate::metadata_lookup::lookup(&DesktopFiles, key, roots, locale)
}
#[cfg(test)]
fn icon(name: &str, roots: &[PathBuf], theme: Option<&str>) -> Option<String> {
    crate::metadata_lookup::icon(&DesktopFiles, name, roots, theme)
}
#[cfg(test)]
use crate::metadata_lookup::{bounded, parse_desktop, safe_name, theme_from_settings};

#[cfg(test)]
#[path = "application_metadata_tests.rs"]
mod tests;
