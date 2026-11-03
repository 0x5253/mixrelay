//! Translate native parameter metadata into the rack contract.

use truce_rack_core::info::ParameterInfo;
use vst3::Steinberg::Vst::{ParameterInfo as Vst3ParameterInfo, ParameterInfo_::ParameterFlags_};

pub(crate) fn empty_parameter_info() -> Vst3ParameterInfo {
    Vst3ParameterInfo {
        id: 0,
        title: [0; 128],
        shortTitle: [0; 128],
        units: [0; 128],
        stepCount: 0,
        defaultNormalizedValue: 0.0,
        unitId: 0,
        flags: 0,
    }
}

pub(super) fn vst3_param_info_to_rack(info: &Vst3ParameterInfo) -> ParameterInfo {
    let name = string128_to_string(&info.title);
    let short_name = string128_to_string(&info.shortTitle);
    let unit = string128_to_string(&info.units);
    let mut flags = truce_rack_core::info::ParameterFlags::empty();
    if info.flags & ParameterFlags_::kIsBypass != 0 {
        flags |= truce_rack_core::info::ParameterFlags::BYPASS;
    }
    if info.flags & ParameterFlags_::kCanAutomate != 0 {
        flags |= truce_rack_core::info::ParameterFlags::AUTOMATABLE;
    }
    if info.flags & ParameterFlags_::kIsHidden != 0 {
        flags |= truce_rack_core::info::ParameterFlags::HIDDEN;
    }
    if info.flags & ParameterFlags_::kIsReadOnly != 0 {
        flags |= truce_rack_core::info::ParameterFlags::READ_ONLY;
    }
    if info.flags & ParameterFlags_::kIsList != 0 {
        flags |= truce_rack_core::info::ParameterFlags::ENUMERATED;
    }
    ParameterInfo {
        id: info.id,
        name,
        short_name,
        unit,
        min: 0.0,
        max: 1.0,
        default: info.defaultNormalizedValue,
        step_count: u32::try_from(info.stepCount).unwrap_or(0),
        flags,
    }
}

/// VST3 `String128` is `[char16; 128]` (UTF-16). Walk the slice
/// until the first NUL and decode to UTF-8.
pub(super) fn string128_to_string(buf: &[u16; 128]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}
