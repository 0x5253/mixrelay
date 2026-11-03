//! Audio Unit parameter metadata, identifiers and factory presets.

use objc2_audio_toolbox::{
    AUPreset, AudioComponentInstance, AudioUnitGetProperty, AudioUnitGetPropertyInfo,
    AudioUnitParameterID, AudioUnitParameterInfo, kAudioUnitProperty_FactoryPresets,
    kAudioUnitProperty_ParameterInfo, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global,
};
use std::ptr;
use truce_rack_core::error::{Error, Result};
use truce_rack_core::info::{ParameterInfo, PresetInfo};

/// Query `kAudioUnitProperty_ParameterInfo` for one parameter.
pub(super) unsafe fn fetch_parameter_info(
    unit: AudioComponentInstance,
    id: AudioUnitParameterID,
) -> Result<AudioUnitParameterInfo> {
    let mut info = AudioUnitParameterInfo {
        name: [0; 52],
        unitName: ptr::null(),
        clumpID: 0,
        cfNameString: ptr::null(),
        unit: objc2_audio_toolbox::AudioUnitParameterUnit::Generic,
        minValue: 0.0,
        maxValue: 0.0,
        defaultValue: 0.0,
        flags: objc2_audio_toolbox::AudioUnitParameterOptions::empty(),
    };
    #[allow(clippy::cast_possible_truncation)]
    let mut size = std::mem::size_of::<AudioUnitParameterInfo>() as u32;
    let status = unsafe {
        AudioUnitGetProperty(
            unit,
            kAudioUnitProperty_ParameterInfo,
            kAudioUnitScope_Global,
            id,
            ptr::NonNull::new_unchecked((&raw mut info).cast()),
            ptr::NonNull::new_unchecked(&raw mut size),
        )
    };
    if status != 0 {
        return Err(Error::Other(format!(
            "kAudioUnitProperty_ParameterInfo failed: OSStatus {status}"
        )));
    }
    Ok(info)
}

#[allow(clippy::cast_sign_loss)]
pub(super) fn au_parameter_to_rack(
    id: AudioUnitParameterID,
    info: &AudioUnitParameterInfo,
) -> ParameterInfo {
    // CFNameString takes precedence over the 52-byte ASCII name
    // field — AU plugins set the latter to empty more often than
    // not since the introduction of CFString-based names.
    let name = if info.cfNameString.is_null() {
        // SAFETY: `info.name` is a NUL-terminated 52-byte ASCII
        // buffer per AU spec.
        let bytes: Vec<u8> = info
            .name
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        // SAFETY: `info.cfNameString` is a +0 reference per AU
        // spec — we don't release it.
        let s = unsafe { &*info.cfNameString };
        s.to_string()
    };
    let unit_name = if info.unitName.is_null() {
        String::new()
    } else {
        let s = unsafe { &*info.unitName };
        s.to_string()
    };
    let flags = au_param_flags_to_rack(info);
    ParameterInfo {
        id,
        name: name.clone(),
        short_name: name,
        unit: unit_name,
        min: f64::from(info.minValue),
        max: f64::from(info.maxValue),
        default: f64::from(info.defaultValue),
        step_count: 0,
        flags,
    }
}

pub(super) fn au_param_flags_to_rack(
    info: &AudioUnitParameterInfo,
) -> truce_rack_core::info::ParameterFlags {
    use objc2_audio_toolbox::AudioUnitParameterOptions as Opt;
    let mut flags = truce_rack_core::info::ParameterFlags::empty();
    // AU treats most parameters as automatable by default; only
    // explicit MeterReadOnly takes that away.
    if info.flags.contains(Opt::Flag_MeterReadOnly) {
        flags |= truce_rack_core::info::ParameterFlags::READ_ONLY;
    } else {
        flags |= truce_rack_core::info::ParameterFlags::AUTOMATABLE;
    }
    if info.flags.contains(Opt::Flag_OmitFromPresets) {
        flags |= truce_rack_core::info::ParameterFlags::HIDDEN;
    }
    flags
}

/// Query `kAudioUnitProperty_FactoryPresets` and return one
/// [`PresetInfo`] per preset. The property hands us a `CFArrayRef`
/// of `AUPreset` structs — we copy the fields out and release the
/// array.
pub(super) unsafe fn fetch_factory_presets(
    unit: AudioComponentInstance,
) -> Option<Vec<PresetInfo>> {
    use objc2_core_foundation::{CFArray, CFRetained};
    let mut presets: *const CFArray = ptr::null();
    let mut size = u32::try_from(std::mem::size_of::<*const CFArray>()).unwrap_or(0);
    let status = unsafe {
        AudioUnitGetProperty(
            unit,
            kAudioUnitProperty_FactoryPresets,
            kAudioUnitScope_Global,
            0,
            ptr::NonNull::new_unchecked((&raw mut presets).cast()),
            ptr::NonNull::new_unchecked(&raw mut size),
        )
    };
    if status != 0 || presets.is_null() {
        return None;
    }
    // CFArrayRef from FactoryPresets is owned per Apple docs —
    // we wrap in CFRetained so it's released on drop.
    let array = unsafe { CFRetained::from_raw(ptr::NonNull::new_unchecked(presets.cast_mut())) };
    let count = array.count();
    if count <= 0 {
        return Some(Vec::new());
    }
    #[allow(clippy::cast_sign_loss)]
    let count_usize = count as usize;
    let mut out = Vec::with_capacity(count_usize);
    for i in 0..count {
        // CFArrayGetValueAtIndex returns *const c_void — points
        // at an `AUPreset` struct living inside the CFArray.
        let value = unsafe { array.value_at_index(i) };
        if value.is_null() {
            continue;
        }
        let preset = unsafe { &*value.cast::<AUPreset>() };
        let name = if preset.presetName.is_null() {
            format!("Preset {}", preset.presetNumber)
        } else {
            unsafe { &*preset.presetName }.to_string()
        };
        #[allow(clippy::cast_sign_loss)]
        out.push(PresetInfo {
            index: i as usize,
            name,
            preset_number: preset.presetNumber,
        });
    }
    Some(out)
}

/// Query `kAudioUnitProperty_ParameterList` and return the
/// parameter ids in declaration order. Returns an empty vec on
/// any failure — a plugin with zero parameters is valid.
pub(super) unsafe fn fetch_parameter_ids(
    unit: AudioComponentInstance,
) -> Vec<AudioUnitParameterID> {
    let mut size: u32 = 0;
    let info_status = unsafe {
        AudioUnitGetPropertyInfo(
            unit,
            kAudioUnitProperty_ParameterList,
            kAudioUnitScope_Global,
            0,
            &raw mut size,
            ptr::null_mut(),
        )
    };
    if info_status != 0 || size == 0 {
        return Vec::new();
    }
    let count = (size as usize) / std::mem::size_of::<AudioUnitParameterID>();
    let mut ids: Vec<AudioUnitParameterID> = vec![0; count];
    let mut io_size = size;
    let get_status = unsafe {
        AudioUnitGetProperty(
            unit,
            kAudioUnitProperty_ParameterList,
            kAudioUnitScope_Global,
            0,
            ptr::NonNull::new_unchecked(ids.as_mut_ptr().cast()),
            ptr::NonNull::new_unchecked(&raw mut io_size),
        )
    };
    if get_status != 0 {
        return Vec::new();
    }
    ids.truncate(io_size as usize / std::mem::size_of::<AudioUnitParameterID>());
    ids
}
