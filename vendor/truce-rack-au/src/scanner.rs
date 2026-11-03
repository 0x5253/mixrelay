//! Registry discovery, component identity and AU catalog loading.

use crate::{AuPlugin, FORMAT};
use objc2_audio_toolbox::{
    AudioComponent, AudioComponentCopyName, AudioComponentDescription, AudioComponentFindNext,
    AudioComponentFlags, kAudioUnitType_Effect, kAudioUnitType_Generator,
    kAudioUnitType_MIDIProcessor, kAudioUnitType_Mixer, kAudioUnitType_MusicDevice,
    kAudioUnitType_MusicEffect,
};
use objc2_core_foundation::CFString;
use std::path::Path;
use std::ptr;
use truce_rack_core::bus::BusLayout;
use truce_rack_core::error::{Error, Result};
use truce_rack_core::info::{PluginCategory, PluginInfo};
use truce_rack_core::scanner::PluginScanner;

/// AU type families we surface to hosts. Output / `FormatConverter`
/// are intentionally skipped — they're system plumbing, not
/// effects.
const SCAN_TYPES: &[u32] = &[
    kAudioUnitType_Effect,
    kAudioUnitType_MusicDevice,
    kAudioUnitType_Generator,
    kAudioUnitType_MusicEffect,
    kAudioUnitType_MIDIProcessor,
    kAudioUnitType_Mixer,
];

/// AU v2 scanner.
#[derive(Debug, Default)]
pub struct AuScanner;

impl AuScanner {
    /// Construct a default scanner.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Load an AU for an already selected catalog layout without enumerating alternatives.
    ///
    /// `layout` must retain the native bus order and roles. Native format acceptance and
    /// initialization are still checked by activation, including after state restoration.
    ///
    /// # Errors
    /// Returns an error for a load failure, invalid topology, or a rejected channel declaration.
    pub fn load_with_layout(&self, info: &PluginInfo, layout: &BusLayout) -> Result<AuPlugin> {
        AuPlugin::load_from(info, Some(layout))
    }
}

impl PluginScanner for AuScanner {
    type Plugin = AuPlugin;

    fn scan(&self) -> Result<Vec<PluginInfo>> {
        let mut out = Vec::new();
        for &type_code in SCAN_TYPES {
            unsafe { scan_family(type_code, &mut out) };
        }
        Ok(out)
    }

    fn scan_path(&self, _path: &Path) -> Result<Vec<PluginInfo>> {
        // AU discovery is registry-based, not path-based.
        Err(Error::Other(
            "truce-rack-au path-bounded scan is not meaningful (AU uses a registry)".into(),
        ))
    }

    fn load(&self, info: &PluginInfo) -> Result<Self::Plugin> {
        AuPlugin::load_from(info, None)
    }
}

pub(super) unsafe fn scan_family(component_type: u32, out: &mut Vec<PluginInfo>) {
    let mut desc = AudioComponentDescription {
        componentType: component_type,
        componentSubType: 0,
        componentManufacturer: 0,
        componentFlags: 0,
        componentFlagsMask: 0,
    };
    let mut component: AudioComponent = ptr::null_mut();
    let category = type_category(component_type);
    let accepts_midi =
        component_type != kAudioUnitType_Effect && component_type != kAudioUnitType_Generator;
    loop {
        let next = unsafe {
            AudioComponentFindNext(component, ptr::NonNull::new_unchecked(&raw mut desc))
        };
        if next.is_null() {
            break;
        }
        component = next;
        let mut comp_desc = AudioComponentDescription {
            componentType: 0,
            componentSubType: 0,
            componentManufacturer: 0,
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        let _ = unsafe {
            objc2_audio_toolbox::AudioComponentGetDescription(
                component,
                ptr::NonNull::new_unchecked(&raw mut comp_desc),
            )
        };
        // Skip AUv3 (app-extension) components — they require
        // AudioComponentInstantiate (async / sandboxed) and are
        // owned by truce-rack-au3. AUv3 components advertise themselves
        // via the IsV3AudioUnit flag in componentFlags.
        if AudioComponentFlags::from_bits_retain(comp_desc.componentFlags)
            .contains(AudioComponentFlags::IsV3AudioUnit)
        {
            continue;
        }
        let name = unsafe { component_name(component) };
        let (vendor, display) = split_name(&name);
        out.push(PluginInfo {
            name: display,
            vendor,
            version: unsafe { component_version(component) },
            category,
            path: std::path::PathBuf::new(),
            unique_id: format_unique_id(&comp_desc),
            format: FORMAT,
            has_editor: false,
            accepts_midi,
        });
    }
}

pub(super) fn type_category(type_code: u32) -> PluginCategory {
    match type_code {
        t if t == kAudioUnitType_MusicDevice => PluginCategory::Instrument,
        t if t == kAudioUnitType_MIDIProcessor => PluginCategory::NoteEffect,
        t if t == kAudioUnitType_Mixer => PluginCategory::Tool,
        _ => PluginCategory::Effect,
    }
}

/// AU `AudioComponentCopyName` returns `"<vendor>: <name>"`. Split
/// it so the host's browser shows them in separate columns.
pub(super) fn split_name(full: &str) -> (String, String) {
    full.split_once(": ").map_or_else(
        || (String::new(), full.to_string()),
        |(v, n)| (v.to_string(), n.to_string()),
    )
}

pub(super) unsafe fn component_name(component: AudioComponent) -> String {
    let mut cf_str: *const CFString = ptr::null();
    let status =
        unsafe { AudioComponentCopyName(component, ptr::NonNull::new_unchecked(&raw mut cf_str)) };
    if status != 0 || cf_str.is_null() {
        return String::new();
    }
    // SAFETY: AudioComponentCopyName follows the "Copy" rule —
    // we own the +1 retain on cf_str. Wrap it in CFRetained so it
    // gets released on drop.
    let retained = unsafe {
        objc2_core_foundation::CFRetained::from_raw(ptr::NonNull::new_unchecked(cf_str.cast_mut()))
    };
    retained.to_string()
}

pub(super) unsafe fn component_version(component: AudioComponent) -> u32 {
    let mut version: u32 = 0;
    let _ = unsafe {
        objc2_audio_toolbox::AudioComponentGetVersion(
            component,
            ptr::NonNull::new_unchecked(&raw mut version),
        )
    };
    version
}

pub(super) fn format_unique_id(desc: &AudioComponentDescription) -> String {
    format!(
        "{}:{}:{}",
        four_cc(desc.componentType),
        four_cc(desc.componentSubType),
        four_cc(desc.componentManufacturer),
    )
}

/// Render a four-CC as its printable ASCII representation if all
/// bytes are printable, otherwise its hex. AU manufacturer codes
/// are conventionally ASCII (`"appl"`, `"vlse"`, …).
pub(super) fn four_cc(code: u32) -> String {
    let bytes = code.to_be_bytes();
    if bytes.iter().all(|b| b.is_ascii_graphic() && *b != b':') {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        format!("{code:08x}")
    }
}

/// Parse a `"type:sub:mfr"` unique id back into a description for
/// `AudioComponentFindNext`.
pub(super) fn parse_unique_id(id: &str) -> Option<AudioComponentDescription> {
    let mut parts = id.split(':');
    let t = parse_four_cc(parts.next()?)?;
    let s = parse_four_cc(parts.next()?)?;
    let m = parse_four_cc(parts.next()?)?;
    Some(AudioComponentDescription {
        componentType: t,
        componentSubType: s,
        componentManufacturer: m,
        componentFlags: 0,
        componentFlagsMask: 0,
    })
}

pub(super) fn parse_four_cc(s: &str) -> Option<u32> {
    if s.len() == 4 && s.bytes().all(|b| b.is_ascii_graphic()) {
        let b = s.as_bytes();
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    } else if s.len() == 8 {
        u32::from_str_radix(s, 16).ok()
    } else {
        None
    }
}
