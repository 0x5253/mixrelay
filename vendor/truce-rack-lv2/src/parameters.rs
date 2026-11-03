//! Cached LV2 control-port metadata. Native calls are confined to load time.

use std::ffi::CStr;

use truce_rack_core::error::{Error, Result};
use truce_rack_core::info::{ParameterFlags, ParameterInfo};

use crate::ports::{PortInfo, PortKind};

const TOGGLED: &[u8] = b"http://lv2plug.in/ns/lv2core#toggled\0";
const INTEGER: &[u8] = b"http://lv2plug.in/ns/lv2core#integer\0";
const SAMPLE_RATE: &[u8] = b"http://lv2plug.in/ns/lv2core#sampleRate\0";
const ENUMERATION: &[u8] = b"http://lv2plug.in/ns/lv2core#enumeration\0";
const UNIT: &[u8] = b"http://lv2plug.in/ns/extensions/units#unit\0";
const UNIT_SYMBOL: &[u8] = b"http://lv2plug.in/ns/extensions/units#symbol\0";

struct Parameter {
    info: ParameterInfo,
    offset: usize,
    toggled: bool,
    integer: bool,
    sample_rate_bounds: Option<(f64, f64)>,
    scale_points: Vec<(f64, String)>,
}

/// Host indices enumerate usable control ports; each native ID remains its LV2 port index.
pub(super) struct Parameters {
    entries: Vec<Parameter>,
    sample_rate: Option<f64>,
}

impl Parameters {
    /// Copies metadata from a retained Lilv world and plugin on the load thread.
    ///
    /// `ports` and `offsets` are the matching results of `classify_ports`. Invalid or
    /// unbounded individual controls are omitted without rejecting the whole plugin.
    ///
    /// # Safety
    /// `world` and `plugin` must be non-null, belong to one Lilv world, and remain live
    /// for this call. The caller owns both and serializes metadata access with loading.
    pub(super) unsafe fn collect(
        world: *mut lilv_sys::LilvWorld,
        plugin: *const lilv_sys::LilvPlugin,
        ports: &[PortInfo],
        offsets: &[usize],
    ) -> Result<Self> {
        if world.is_null() || plugin.is_null() || ports.len() != offsets.len() {
            return Err(Error::Other("invalid LV2 parameter metadata source".into()));
        }
        // SAFETY: the caller retains the matching world/plugin throughout this load-time read.
        let uris = unsafe { Uris::new(world) }
            .ok_or_else(|| Error::Other("could not allocate LV2 property URIs".into()))?;
        let count = ports.len();
        let mut mins = vec![f32::NAN; count];
        let mut maxes = vec![f32::NAN; count];
        let mut defaults = vec![f32::NAN; count];
        // SAFETY: classify_ports produced one entry for every native port, and all arrays
        // remain writable for that count during the Lilv call.
        unsafe {
            lilv_sys::lilv_plugin_get_port_ranges_float(
                plugin,
                mins.as_mut_ptr(),
                maxes.as_mut_ptr(),
                defaults.as_mut_ptr(),
            );
        }
        let mut entries = Vec::new();
        for (position, port_info) in ports.iter().enumerate() {
            if !matches!(port_info.kind, PortKind::Control { .. })
                || offsets[position] == usize::MAX
            {
                continue;
            }
            let Some((min, max)) = finite_range(mins[position], maxes[position]) else {
                continue;
            };
            // SAFETY: port indices were copied from this plugin by classify_ports.
            let port = unsafe { lilv_sys::lilv_plugin_get_port_by_index(plugin, port_info.index) };
            if port.is_null() {
                continue;
            }
            // SAFETY: the world, plugin, port, and URI nodes remain live for this iteration.
            let native = NativePort {
                world,
                plugin,
                port,
            };
            let source = PortMetadata {
                info: port_info,
                offset: offsets[position],
                range: (min, max, defaults[position]),
            };
            let entry = unsafe { Parameter::collect(native, source, &uris) };
            entries.push(entry);
        }
        Ok(Self {
            entries,
            sample_rate: None,
        })
    }

    /// Number of cached host parameters, including bounded read-only output controls.
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Native-unit metadata for a contiguous host index.
    pub(super) fn info(&self, index: usize) -> Result<ParameterInfo> {
        Ok(self.entry(index)?.info.clone())
    }

    /// Read the same scalar storage connected to DSP, UI, and LV2 state.
    pub(super) fn value(&self, index: usize, values: &[f32]) -> Result<f64> {
        let entry = self.entry(index)?;
        values
            .get(entry.offset)
            .copied()
            .map(f64::from)
            .ok_or(Error::InvalidParameter(index))
    }

    /// Offset of the connected scalar for a host parameter index.
    pub(super) fn offset(&self, index: usize) -> Result<usize> {
        Ok(self.entry(index)?.offset)
    }

    /// Format a native value, using declared scale-point labels when available.
    pub(super) fn formatted(&self, index: usize, value: f64) -> Result<String> {
        let entry = self.entry(index)?;
        if !value.is_finite() {
            return Err(Error::InvalidParameter(index));
        }
        if entry.toggled {
            return Ok(if value > 0.0 { "On" } else { "Off" }.into());
        }
        if let Some((_, label)) = entry
            .scale_points
            .iter()
            .find(|(point, _)| exact_native_value(*point, value))
        {
            return Ok(label.clone());
        }
        Ok(format!("{value}"))
    }

    /// Set a bounded writable input in native units, preserving the shared f32 slot.
    pub(super) fn set(&self, index: usize, value: f64, values: &mut [f32]) -> Result<()> {
        let entry = self.entry(index)?;
        if entry.sample_rate_bounds.is_some() && self.sample_rate.is_none() {
            return Err(Error::NotActivated);
        }
        if entry.info.flags.contains(ParameterFlags::READ_ONLY)
            || !value.is_finite()
            || value < entry.info.min
            || value > entry.info.max
        {
            return Err(Error::InvalidParameter(index));
        }
        // LV2 integer/toggle properties are UI hints; quantize host writes to their discrete
        // scalar representation without treating the declared native range as normalized.
        let native = if entry.toggled {
            if value > 0.0 { 1.0 } else { 0.0 }
        } else if entry.integer {
            value.round()
        } else {
            value
        };
        if native < entry.info.min || native > entry.info.max {
            return Err(Error::InvalidParameter(index));
        }
        let scalar = native as f32;
        if !scalar.is_finite() {
            return Err(Error::InvalidParameter(index));
        }
        *values
            .get_mut(entry.offset)
            .ok_or(Error::InvalidParameter(index))? = scalar;
        Ok(())
    }

    /// Scale only `lv2:sampleRate` bounds after the actual instance rate is known.
    /// The declared default and connected scalar are already native control values.
    pub(super) fn set_sample_rate(&mut self, rate: f64) -> Result<()> {
        if !rate.is_finite() || rate <= 0.0 {
            return Err(Error::Other("invalid LV2 sample rate".into()));
        }
        if self.entries.iter().any(|entry| {
            entry
                .sample_rate_bounds
                .is_some_and(|(min, max)| !(min * rate).is_finite() || !(max * rate).is_finite())
        }) {
            return Err(Error::Other("LV2 sample-rate bounds overflow".into()));
        }
        for entry in &mut self.entries {
            let Some((min, max)) = entry.sample_rate_bounds else {
                continue;
            };
            entry.info.min = min * rate;
            entry.info.max = max * rate;
            if entry.integer {
                entry.info.step_count = integer_steps(entry.info.min, entry.info.max);
            }
        }
        self.sample_rate = Some(rate);
        Ok(())
    }

    fn entry(&self, index: usize) -> Result<&Parameter> {
        self.entries
            .get(index)
            .ok_or(Error::InvalidParameter(index))
    }
}

fn finite_range(min: f32, max: f32) -> Option<(f64, f64)> {
    (min.is_finite() && max.is_finite() && min <= max).then(|| (f64::from(min), f64::from(max)))
}

#[derive(Clone, Copy)]
struct NativePort {
    world: *mut lilv_sys::LilvWorld,
    plugin: *const lilv_sys::LilvPlugin,
    port: *const lilv_sys::LilvPort,
}

#[derive(Clone, Copy)]
struct PortMetadata<'a> {
    info: &'a PortInfo,
    offset: usize,
    range: (f64, f64, f32),
}

struct Uri(*mut lilv_sys::LilvNode);

impl Uri {
    unsafe fn new(world: *mut lilv_sys::LilvWorld, bytes: &[u8]) -> Option<Self> {
        // SAFETY: the static bytes are NUL-terminated and `world` is retained by the caller.
        let node = unsafe { lilv_sys::lilv_new_uri(world, bytes.as_ptr().cast()) };
        (!node.is_null()).then_some(Self(node))
    }
}

impl Drop for Uri {
    fn drop(&mut self) {
        // SAFETY: this unique node came from lilv_new_uri and is freed once here.
        unsafe { lilv_sys::lilv_node_free(self.0) };
    }
}

struct Uris {
    toggled: Uri,
    integer: Uri,
    sample_rate: Uri,
    enumeration: Uri,
    unit: Uri,
    unit_symbol: Uri,
}

impl Uris {
    unsafe fn new(world: *mut lilv_sys::LilvWorld) -> Option<Self> {
        // SAFETY: all constants are NUL-terminated; partial construction drops earlier nodes.
        Some(Self {
            toggled: unsafe { Uri::new(world, TOGGLED) }?,
            integer: unsafe { Uri::new(world, INTEGER) }?,
            sample_rate: unsafe { Uri::new(world, SAMPLE_RATE) }?,
            enumeration: unsafe { Uri::new(world, ENUMERATION) }?,
            unit: unsafe { Uri::new(world, UNIT) }?,
            unit_symbol: unsafe { Uri::new(world, UNIT_SYMBOL) }?,
        })
    }
}

impl Parameter {
    unsafe fn collect(native: NativePort, source: PortMetadata<'_>, uris: &Uris) -> Self {
        // SAFETY: all pointers are retained by Parameters::collect for this call.
        let symbol = unsafe { lilv_sys::lilv_port_get_symbol(native.plugin, native.port) };
        let symbol = unsafe { node_string(symbol) }.unwrap_or_default();
        let name_node = unsafe { lilv_sys::lilv_port_get_name(native.plugin, native.port) };
        let name = unsafe { node_string(name_node) }.unwrap_or_else(|| symbol.clone());
        unsafe { lilv_sys::lilv_node_free(name_node) };
        let property = |uri: &Uri| unsafe {
            lilv_sys::lilv_port_has_property(native.plugin, native.port, uri.0)
        };
        let toggled = property(&uris.toggled);
        let integer = property(&uris.integer);
        let sample_rate = property(&uris.sample_rate);
        let enumerated = property(&uris.enumeration);
        let scale_points = if enumerated {
            unsafe { scale_points(native.plugin, native.port) }
        } else {
            Vec::new()
        };
        let mut flags = if source.info.is_input {
            ParameterFlags::AUTOMATABLE
        } else {
            ParameterFlags::READ_ONLY
        };
        if enumerated && !scale_points.is_empty() {
            flags |= ParameterFlags::ENUMERATED;
        }
        let default = if source.range.2.is_finite() {
            f64::from(source.range.2)
        } else if let PortKind::Control { default } = source.info.kind {
            f64::from(default)
        } else {
            0.0
        };
        let unit = unsafe { unit_symbol(native.world, native.plugin, native.port, uris) };
        let step_count = if toggled {
            2
        } else if integer {
            integer_steps(source.range.0, source.range.1)
        } else if flags.contains(ParameterFlags::ENUMERATED) {
            u32::try_from(scale_points.len()).unwrap_or(u32::MAX)
        } else {
            0
        };
        Self {
            info: ParameterInfo {
                id: source.info.index,
                name,
                short_name: symbol,
                unit,
                min: source.range.0,
                max: source.range.1,
                default,
                step_count,
                flags,
            },
            offset: source.offset,
            toggled,
            integer,
            sample_rate_bounds: sample_rate.then_some((source.range.0, source.range.1)),
            scale_points,
        }
    }
}

fn integer_steps(min: f64, max: f64) -> u32 {
    let count = max.floor() - min.ceil() + 1.0;
    if count >= 1.0 && count <= f64::from(u32::MAX) {
        // `count` is an integer formed from finite floor/ceil values and is bounded by
        // u32::MAX, well inside i64's exact f64 range. The signed cast avoids sign loss.
        u32::try_from(count as i64).expect("validated LV2 integer step count must fit u32")
    } else {
        0
    }
}

fn exact_native_value(left: f64, right: f64) -> bool {
    // Native scale-point labels require exact identity, except IEEE signed zeroes
    // compare equal in the original LV2 value semantics.
    const SIGNLESS: u64 = 0x7fff_ffff_ffff_ffff;
    let canonical = |value: f64| {
        let bits = value.to_bits();
        if bits & SIGNLESS == 0 { 0 } else { bits }
    };
    canonical(left) == canonical(right)
}

unsafe fn node_string(node: *const lilv_sys::LilvNode) -> Option<String> {
    if node.is_null() {
        return None;
    }
    // SAFETY: Lilv owns this live node and the returned C string remains valid for this read.
    let string = unsafe { lilv_sys::lilv_node_as_string(node) };
    (!string.is_null()).then(|| {
        unsafe { CStr::from_ptr(string) }
            .to_string_lossy()
            .into_owned()
    })
}

unsafe fn unit_symbol(
    world: *mut lilv_sys::LilvWorld,
    plugin: *const lilv_sys::LilvPlugin,
    port: *const lilv_sys::LilvPort,
    uris: &Uris,
) -> String {
    let unit = unsafe { lilv_sys::lilv_port_get(plugin, port, uris.unit.0) };
    if unit.is_null() {
        return String::new();
    }
    let symbol =
        unsafe { lilv_sys::lilv_world_get(world, unit, uris.unit_symbol.0, std::ptr::null()) };
    let text = unsafe { node_string(symbol) }.unwrap_or_default();
    unsafe {
        lilv_sys::lilv_node_free(symbol);
        lilv_sys::lilv_node_free(unit);
    }
    text
}

unsafe fn scale_points(
    plugin: *const lilv_sys::LilvPlugin,
    port: *const lilv_sys::LilvPort,
) -> Vec<(f64, String)> {
    let points = unsafe { lilv_sys::lilv_port_get_scale_points(plugin, port) };
    if points.is_null() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cursor = unsafe { lilv_sys::lilv_scale_points_begin(points) };
    while !unsafe { lilv_sys::lilv_scale_points_is_end(points, cursor) } {
        let point = unsafe { lilv_sys::lilv_scale_points_get(points, cursor) };
        if point.is_null() {
            cursor = unsafe { lilv_sys::lilv_scale_points_next(points, cursor) };
            continue;
        }
        let value = unsafe { lilv_sys::lilv_scale_point_get_value(point) };
        let label = unsafe { lilv_sys::lilv_scale_point_get_label(point) };
        let numeric = unsafe { node_number(value) };
        if let (Some(numeric), Some(label)) = (numeric, unsafe { node_string(label) }) {
            out.push((numeric, label));
        }
        cursor = unsafe { lilv_sys::lilv_scale_points_next(points, cursor) };
    }
    unsafe { lilv_sys::lilv_scale_points_free(points) };
    out
}

unsafe fn node_number(node: *const lilv_sys::LilvNode) -> Option<f64> {
    if node.is_null() {
        return None;
    }
    let value = if unsafe { lilv_sys::lilv_node_is_float(node) } {
        f64::from(unsafe { lilv_sys::lilv_node_as_float(node) })
    } else if unsafe { lilv_sys::lilv_node_is_int(node) } {
        f64::from(unsafe { lilv_sys::lilv_node_as_int(node) })
    } else {
        return None;
    };
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Lv2Scanner;
    use truce_rack_core::plugin::PluginCore;
    use truce_rack_core::scanner::PluginScanner;

    fn parameter(min: f64, max: f64, writable: bool) -> Parameters {
        Parameters {
            entries: vec![Parameter {
                info: ParameterInfo {
                    id: 7,
                    name: "Gain".into(),
                    short_name: "gain".into(),
                    unit: "dB".into(),
                    min,
                    max,
                    default: 0.0,
                    step_count: 0,
                    flags: if writable {
                        ParameterFlags::AUTOMATABLE
                    } else {
                        ParameterFlags::READ_ONLY
                    },
                },
                offset: 1,
                toggled: false,
                integer: false,
                sample_rate_bounds: None,
                scale_points: Vec::new(),
            }],
            sample_rate: None,
        }
    }

    #[test]
    fn host_index_preserves_native_port_id_and_shared_scalar_offset() {
        let params = parameter(-12.0, 12.0, true);
        let mut values = [10.0, 0.0];
        assert_eq!(params.len(), 1);
        assert_eq!(params.info(0).unwrap().id, 7);
        params.set(0, -6.0, &mut values).unwrap();
        assert_eq!(
            values.map(f32::to_bits),
            [10.0_f32.to_bits(), (-6.0_f32).to_bits()]
        );
        assert_eq!(
            params.value(0, &values).unwrap().to_bits(),
            (-6.0_f64).to_bits()
        );
    }

    #[test]
    fn setter_rejects_unwritable_and_invalid_values_without_mutating_scalar() {
        let writable = parameter(-12.0, 12.0, true);
        let readonly = parameter(-12.0, 12.0, false);
        let mut values = [0.0, 3.0];
        for value in [f64::NAN, f64::INFINITY, -13.0, 13.0] {
            assert!(writable.set(0, value, &mut values).is_err());
        }
        assert!(readonly.set(0, 0.0, &mut values).is_err());
        assert!(writable.set(1, 0.0, &mut values).is_err());
        assert_eq!(
            values.map(f32::to_bits),
            [0.0_f32.to_bits(), 3.0_f32.to_bits()]
        );
    }

    #[test]
    fn declared_integer_and_toggle_hints_keep_native_bounds() {
        assert_eq!(integer_steps(-2.0, 2.0), 5);
        assert_eq!(integer_steps(0.25, 2.75), 2);
        assert_eq!(integer_steps(0.0, f64::from(u32::MAX - 1)), u32::MAX);
        assert_eq!(integer_steps(0.0, f64::from(u32::MAX)), 0);
        assert_eq!(integer_steps(0.0, f64::MAX), 0);
        let mut params = parameter(0.0, 0.5, true);
        params.entries[0].sample_rate_bounds = Some((0.0, 0.5));
        params.set_sample_rate(48_000.0).unwrap();
        assert_eq!(
            params.info(0).unwrap().max.to_bits(),
            24_000.0_f64.to_bits()
        );
        assert_eq!(params.info(0).unwrap().default.to_bits(), 0.0_f64.to_bits());
        params.entries[0].integer = true;
        let mut values = [0.0, 0.0];
        params.set(0, 12_000.6, &mut values).unwrap();
        assert_eq!(values[1].to_bits(), 12_001.0_f32.to_bits());
        params.entries[0].toggled = true;
        params.set(0, 0.5, &mut values).unwrap();
        assert_eq!(values[1].to_bits(), 1.0_f32.to_bits());
    }

    #[test]
    fn scale_point_label_is_used_without_changing_the_native_value() {
        let mut params = parameter(0.0, 2.0, true);
        params.entries[0].scale_points.push((2.0, "Fast".into()));
        params.entries[0].scale_points.push((0.0, "Zero".into()));
        assert_eq!(params.formatted(0, 2.0).unwrap(), "Fast");
        assert_eq!(params.formatted(0, -0.0).unwrap(), "Zero");
        assert!(!exact_native_value(
            1.0,
            f64::from_bits(1.0_f64.to_bits() + 1)
        ));
        assert_eq!(params.formatted(0, 1.0).unwrap(), "1");
    }

    #[test]
    fn native_state_fixture_exposes_its_bounded_input_control() {
        use std::path::Path;

        let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/state.lv2");
        let info = Lv2Scanner::new()
            .scan_path(&bundle)
            .unwrap()
            .into_iter()
            .find(|info| info.unique_id == "urn:mixrelay:state-fixture")
            .unwrap();
        let plugin = Lv2Scanner::new().load(&info).unwrap();
        assert_eq!(plugin.parameter_count(), 1);
        let gain = plugin.parameter_info(0).unwrap();
        assert_eq!(gain.id, 2);
        assert_eq!(gain.name, "Gain");
        assert_eq!(gain.short_name, "gain");
        assert_eq!(
            (
                gain.min.to_bits(),
                gain.max.to_bits(),
                gain.default.to_bits()
            ),
            (0.0_f64.to_bits(), 1.0_f64.to_bits(), 1.0_f64.to_bits())
        );
        assert!(gain.flags.contains(ParameterFlags::AUTOMATABLE));
        assert_eq!(
            plugin.parameter_value(0).unwrap().to_bits(),
            1.0_f64.to_bits()
        );
    }
}
