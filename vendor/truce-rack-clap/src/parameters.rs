//! Parameter metadata, pending values, and main-thread flush.

use crate::events::{ConvertedInputEvents, OutputEventsSink};
use crate::host::HostContext;
use crate::parameter_ids::{ParameterIds, ParameterMetadata};
use clap_sys::ext::params::{
    CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_BYPASS, CLAP_PARAM_IS_ENUM, CLAP_PARAM_IS_HIDDEN,
    CLAP_PARAM_IS_READONLY, CLAP_PARAM_IS_STEPPED, CLAP_PARAM_REQUIRES_PROCESS, clap_param_info,
    clap_plugin_params,
};
use clap_sys::plugin::clap_plugin;
use clap_sys::process::CLAP_PROCESS_ERROR;
use std::ffi::{CStr, c_char};
use std::ptr;
use truce_rack_core::error::{Error, Result};
use truce_rack_core::info::ParameterInfo;

pub(super) struct Parameters {
    pub(super) ids: ParameterIds,
    pub(super) changed_pending: bool,
    pub(super) extension: *const clap_plugin_params,
}
pub(super) fn empty_param_info() -> clap_param_info {
    clap_param_info {
        id: 0,
        flags: 0,
        cookie: ptr::null_mut(),
        name: [0; clap_sys::string_sizes::CLAP_NAME_SIZE],
        module: [0; clap_sys::string_sizes::CLAP_PATH_SIZE],
        min_value: 0.0,
        max_value: 0.0,
        default_value: 0.0,
    }
}

pub(super) fn clap_param_info_to_rack(info: &clap_param_info) -> ParameterInfo {
    let name = c_buf_to_string(&info.name);
    let mut flags = truce_rack_core::info::ParameterFlags::empty();
    if info.flags & CLAP_PARAM_IS_BYPASS != 0 {
        flags |= truce_rack_core::info::ParameterFlags::BYPASS;
    }
    if info.flags & CLAP_PARAM_IS_AUTOMATABLE != 0 {
        flags |= truce_rack_core::info::ParameterFlags::AUTOMATABLE;
    }
    if info.flags & CLAP_PARAM_IS_HIDDEN != 0 {
        flags |= truce_rack_core::info::ParameterFlags::HIDDEN;
    }
    if info.flags & CLAP_PARAM_IS_READONLY != 0 {
        flags |= truce_rack_core::info::ParameterFlags::READ_ONLY;
    }
    if info.flags & CLAP_PARAM_IS_ENUM != 0 {
        flags |= truce_rack_core::info::ParameterFlags::ENUMERATED;
    }
    let step_count = if info.flags & CLAP_PARAM_IS_STEPPED != 0 {
        // Stepped parameters have integer-valued [min, max]; the
        // step count is the number of distinct values.
        #[allow(clippy::cast_possible_truncation)]
        let span = (info.max_value - info.min_value).round() as i64;
        span.checked_add(1)
            .and_then(|count| u32::try_from(count).ok())
            .unwrap_or(0)
    } else {
        0
    };
    ParameterInfo {
        id: info.id,
        name: name.clone(),
        short_name: name,
        unit: String::new(), // CLAP doesn't expose a separate unit field
        min: info.min_value,
        max: info.max_value,
        default: info.default_value,
        step_count,
        flags,
    }
}

pub(super) fn c_buf_to_string(buf: &[c_char]) -> String {
    // `c_char` is signed on some targets and unsigned on others. Its native
    // representation is one byte, so this preserves the CLAP byte without a cast.
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&byte| byte.to_ne_bytes()[0])
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

pub(super) unsafe fn cstr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

pub(super) fn read_parameter_ids(
    plugin: *const clap_plugin,
    params_ext: *const clap_plugin_params,
) -> Result<ParameterIds> {
    if params_ext.is_null() {
        return Ok(ParameterIds::empty());
    }
    // SAFETY: the extension pointer was obtained from this live plugin instance and callers
    // invoke this helper only on its fixed main thread while holding exclusive instance access.
    let params = unsafe { &*params_ext };
    let count = params
        .count
        .ok_or_else(|| Error::Other("clap.params missing count callback".into()))?;
    let get_info = params
        .get_info
        .ok_or_else(|| Error::Other("clap.params missing get_info callback".into()))?;
    let count = unsafe { count(plugin) };
    ParameterIds::collect(count, |index| {
        let mut info = empty_param_info();
        // SAFETY: the CLAP parameter index is within the count just returned by this instance;
        // caller is the plugin's main thread and the output record has the C ABI layout.
        if !unsafe { get_info(plugin, index, &raw mut info) } {
            return Err(Error::Other(format!(
                "clap.params.get_info failed while caching parameter index {index}"
            )));
        }
        Ok(ParameterMetadata {
            id: info.id,
            requires_process: info.flags & CLAP_PARAM_REQUIRES_PROCESS != 0,
        })
    })
}

pub(super) fn queue_cached_parameter_change(
    parameter_ids: &mut ParameterIds,
    index: usize,
    value: f64,
) -> Result<()> {
    parameter_ids.queue(index, value)
}

pub(super) fn flush_pending_parameter_changes(
    plugin: *const clap_plugin,
    params_ext: *const clap_plugin_params,
    host_context: &HostContext,
    parameter_ids: &mut ParameterIds,
) -> Result<bool> {
    let _main_thread = host_context.main_thread_scope()?;
    if !parameter_ids.has_flushable_pending() {
        return Ok(false);
    }
    if params_ext.is_null() {
        return Err(Error::Other(
            "pending CLAP parameter changes lost their params extension".into(),
        ));
    }
    let Some(flush) = (unsafe { (*params_ext).flush }) else {
        return Ok(false);
    };

    let mut converted = ConvertedInputEvents { events: Vec::new() };
    parameter_ids.for_each_flushable(|param_id, value| {
        converted.push_param_value(0, param_id, value);
    });
    let input = converted.as_clap();
    let mut sink = OutputEventsSink::new(None);
    let output = sink.as_clap();
    // The fixed-main broker owns the instance exclusively here; CLAP's audio-thread role is
    // symbolic and may be reported on this thread for the serialized flush call.
    {
        let _audio_thread = host_context.audio_thread_scope();
        unsafe { flush(plugin, &raw const input, &raw const output) };
    }
    parameter_ids.clear_flushed();
    Ok(true)
}

pub(super) fn commit_processed_parameter_changes(parameter_ids: &mut ParameterIds, status: i32) {
    if status != CLAP_PROCESS_ERROR {
        parameter_ids.clear_all_pending();
    }
}
