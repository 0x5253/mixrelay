//! Load-time LV2 latency metadata and validated output control snapshots.

use super::{PortInfo, PortKind};

const LV2_CORE_DESIGNATION_URI: &[u8] = b"http://lv2plug.in/ns/lv2core#designation\0";
const LV2_CORE_LATENCY_URI: &[u8] = b"http://lv2plug.in/ns/lv2core#latency\0";
const LV2_CORE_REPORTS_LATENCY_URI: &[u8] = b"http://lv2plug.in/ns/lv2core#reportsLatency\0";
/// Metadata source for the plugin-reported LV2 latency scalar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LatencyPort {
    /// No latency port is reported by the plugin; LV2 defines this as zero latency.
    NotReported,
    /// Connected output control-port offset, read after `lilv_instance_run`.
    OutputControl { offset: usize },
    /// Metadata is malformed, ambiguous, or could not be inspected reliably.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LatencyCandidate {
    NotReported,
    Reported(usize),
    Malformed,
}

struct LatencyMetadataUris {
    designation: *mut lilv_sys::LilvNode,
    latency: *mut lilv_sys::LilvNode,
    reports_latency: *mut lilv_sys::LilvNode,
    control_port: *mut lilv_sys::LilvNode,
    output_port: *mut lilv_sys::LilvNode,
}

impl LatencyMetadataUris {
    /// Create cached nodes for the LV2 latency metadata terms.
    ///
    /// # Safety
    /// `world` must be a live Lilv world that remains alive until the returned guard is dropped.
    unsafe fn new(world: *mut lilv_sys::LilvWorld) -> Self {
        // SAFETY: `world` is the live world that owns the plugin being classified, and every URI
        // byte string is NUL-terminated as required by `lilv_new_uri`.
        unsafe {
            Self {
                designation: lilv_sys::lilv_new_uri(
                    world,
                    LV2_CORE_DESIGNATION_URI.as_ptr().cast(),
                ),
                latency: lilv_sys::lilv_new_uri(world, LV2_CORE_LATENCY_URI.as_ptr().cast()),
                reports_latency: lilv_sys::lilv_new_uri(
                    world,
                    LV2_CORE_REPORTS_LATENCY_URI.as_ptr().cast(),
                ),
                control_port: lilv_sys::lilv_new_uri(
                    world,
                    lilv_sys::LILV_URI_CONTROL_PORT.as_ptr().cast(),
                ),
                output_port: lilv_sys::lilv_new_uri(
                    world,
                    lilv_sys::LILV_URI_OUTPUT_PORT.as_ptr().cast(),
                ),
            }
        }
    }

    fn is_complete(&self) -> bool {
        !self.designation.is_null()
            && !self.latency.is_null()
            && !self.reports_latency.is_null()
            && !self.control_port.is_null()
            && !self.output_port.is_null()
    }
}

impl Drop for LatencyMetadataUris {
    fn drop(&mut self) {
        for uri in [
            self.designation,
            self.latency,
            self.reports_latency,
            self.control_port,
            self.output_port,
        ] {
            if !uri.is_null() {
                // SAFETY: each non-null node is owned by this guard and was returned by
                // `lilv_new_uri`; no node is freed more than once.
                unsafe { lilv_sys::lilv_node_free(uri) };
            }
        }
    }
}

/// Resolve modern `lv2:designation lv2:latency` and deprecated `lv2:reportsLatency` metadata.
///
/// The deprecated property remains in use by third-party plugins, so it is recognized as the
/// same output-port contract. Multiple distinct or malformed reports are unknown rather than
/// silently becoming zero latency.
///
/// # Safety
/// `world` and `plugin` must be live Lilv objects from the same world, and `ports` must be the
/// metadata previously classified from that plugin.
pub(super) unsafe fn classify_latency_port(
    world: *mut lilv_sys::LilvWorld,
    plugin: *const lilv_sys::LilvPlugin,
    ports: &[PortInfo],
    control_value_offset: &[usize],
) -> LatencyPort {
    // SAFETY: the caller guarantees this is the live world for `plugin`.
    let uris = unsafe { LatencyMetadataUris::new(world) };
    if !uris.is_complete() {
        return LatencyPort::Unknown;
    }

    let mut candidate_offset = None;
    let mut malformed = false;
    for info in ports {
        // SAFETY: plugin, port metadata, and URI nodes remain live for this classification call.
        match unsafe { latency_port_candidate(plugin, info, control_value_offset, &uris) } {
            LatencyCandidate::Reported(offset)
                if candidate_offset.is_some_and(|old| old != offset) =>
            {
                malformed = true;
            }
            LatencyCandidate::Reported(offset) => candidate_offset = Some(offset),
            LatencyCandidate::NotReported => {}
            LatencyCandidate::Malformed => malformed = true,
        }
    }
    if malformed {
        LatencyPort::Unknown
    } else if let Some(offset) = candidate_offset {
        LatencyPort::OutputControl { offset }
    } else {
        LatencyPort::NotReported
    }
}

/// Return the connected control offset if this port reports latency, or `Malformed` if invalid.
///
/// # Safety
/// `plugin`, its indexed port, and all nodes in `uris` must remain live for this call.
unsafe fn latency_port_candidate(
    plugin: *const lilv_sys::LilvPlugin,
    info: &PortInfo,
    control_value_offset: &[usize],
    uris: &LatencyMetadataUris,
) -> LatencyCandidate {
    // SAFETY: `info.index` belongs to the live plugin described by the caller.
    let port = unsafe { lilv_sys::lilv_plugin_get_port_by_index(plugin, info.index) };
    if port.is_null() {
        return LatencyCandidate::Malformed;
    }
    // SAFETY: `port` and the predicate/value nodes are valid objects in the same Lilv world.
    let designation = unsafe { lilv_sys::lilv_port_get(plugin, port, uris.designation) };
    let modern_report =
        !designation.is_null() && unsafe { lilv_sys::lilv_node_equals(designation, uris.latency) };
    if !designation.is_null() {
        // SAFETY: `lilv_port_get` returns an owned node that this function frees once.
        unsafe { lilv_sys::lilv_node_free(designation) };
    }
    // SAFETY: `port` and the property node are valid objects in the same Lilv world.
    let legacy_report =
        unsafe { lilv_sys::lilv_port_has_property(plugin, port, uris.reports_latency) };
    if !modern_report && !legacy_report {
        return LatencyCandidate::NotReported;
    }
    // SAFETY: the port and class nodes are valid objects in the same Lilv world.
    let valid_output = !info.is_input
        && matches!(info.kind, PortKind::Control { .. })
        && unsafe { lilv_sys::lilv_port_is_a(plugin, port, uris.control_port) }
        && unsafe { lilv_sys::lilv_port_is_a(plugin, port, uris.output_port) };
    if !valid_output {
        return LatencyCandidate::Malformed;
    }
    let Some(offset) = control_value_offset
        .get(info.index as usize)
        .copied()
        .filter(|offset| *offset != usize::MAX)
    else {
        return LatencyCandidate::Malformed;
    };
    LatencyCandidate::Reported(offset)
}

fn latency_frames_from_lv2_value(value: f32) -> Option<u32> {
    let frames = f64::from(value);
    if !frames.is_finite() || frames < 0.0 || frames.fract() != 0.0 || frames > f64::from(u32::MAX)
    {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(frames as u32)
}

pub(crate) fn latency_snapshot(port: LatencyPort, value: Option<f32>) -> Option<u32> {
    match port {
        LatencyPort::NotReported => Some(0),
        LatencyPort::OutputControl { .. } => value.and_then(latency_frames_from_lv2_value),
        LatencyPort::Unknown => None,
    }
}

#[cfg(test)]
mod tests;
