//! CLAP audio-port configuration discovery and activation checks.

use std::ffi::{CStr, c_char};
use std::fmt::Write as _;
use std::ptr;

use clap_sys::ext::audio_ports::{
    CLAP_AUDIO_PORT_IS_MAIN, CLAP_PORT_MONO, CLAP_PORT_STEREO, clap_audio_port_info,
    clap_plugin_audio_ports,
};
use clap_sys::ext::audio_ports_config::{
    clap_audio_ports_config, clap_plugin_audio_ports_config, clap_plugin_audio_ports_config_info,
};
use clap_sys::id::CLAP_INVALID_ID;
use clap_sys::plugin::clap_plugin;
use clap_sys::string_sizes::CLAP_NAME_SIZE;
use sha2::{Digest, Sha256};
use truce_rack_core::bus::{Bus, BusKind, BusLayout, ChannelConfig};
use truce_rack_core::error::{Error, Result};

const MAX_AUDIO_CONFIGS: u32 = 256;
const MAX_AUDIO_BUSES: u32 = 64;
const MAX_AUDIO_CHANNELS: u32 = 64;

type SelectAudioConfig = unsafe extern "C" fn(*const clap_plugin, clap_sys::id::clap_id) -> bool;

#[derive(Clone, Copy, PartialEq, Eq)]
struct LayoutIdentity {
    config_id: Option<u32>,
    port_ids_fingerprint: [u8; 32],
}

struct PortDescriptions {
    buses: Vec<Bus>,
    native_ids: Vec<u32>,
}

/// Native audio-port APIs and the layouts they expose for one loaded plugin instance.
///
/// The raw pointers are borrowed from the owning `ClapPlugin`; that owner retains the plugin and
/// its dynamic library until this catalog is dropped.
pub(super) struct AudioPortCatalog {
    plugin: *const clap_plugin,
    audio_ports: *const clap_plugin_audio_ports,
    audio_ports_config: *const clap_plugin_audio_ports_config,
    audio_ports_config_info: *const clap_plugin_audio_ports_config_info,
    layouts: Vec<BusLayout>,
}

impl AudioPortCatalog {
    /// Discovers the current CLAP audio topology and every advertised configuration.
    ///
    /// # Safety
    /// `plugin` must be a live, initialized instance. Every non-null extension pointer must have
    /// been returned by that exact instance, remain valid for its lifetime, and be called from
    /// the instance's serialized main thread while the plugin is inactive.
    pub(super) unsafe fn discover(
        plugin: *const clap_plugin,
        audio_ports: *const clap_plugin_audio_ports,
        audio_ports_config: *const clap_plugin_audio_ports_config,
        audio_ports_config_info: *const clap_plugin_audio_ports_config_info,
    ) -> Result<Self> {
        if plugin.is_null() {
            return Err(invalid("plugin instance is null"));
        }
        if !audio_ports_config_info.is_null() && audio_ports_config.is_null() {
            return Err(invalid(
                "clap.audio-ports-config-info exists without clap.audio-ports-config",
            ));
        }

        let mut catalog = Self {
            plugin,
            audio_ports,
            audio_ports_config,
            audio_ports_config_info,
            layouts: Vec::new(),
        };
        catalog.layouts = if audio_ports.is_null() {
            if !audio_ports_config.is_null() {
                return Err(invalid(
                    "audio-ports-config exists without clap.audio-ports",
                ));
            }
            let mut layouts = Vec::new();
            layouts
                .try_reserve_exact(1)
                .map_err(|error| invalid(format!("cannot reserve empty layout: {error}")))?;
            layouts.push(build_layout(
                None,
                PortDescriptions {
                    buses: Vec::new(),
                    native_ids: Vec::new(),
                },
                PortDescriptions {
                    buses: Vec::new(),
                    native_ids: Vec::new(),
                },
            )?);
            layouts
        } else if audio_ports_config.is_null() {
            let layout = catalog.query_active_layout(None)?;
            let mut layouts = Vec::new();
            layouts
                .try_reserve_exact(1)
                .map_err(|error| invalid(format!("cannot reserve current layout: {error}")))?;
            layouts.push(layout);
            layouts
        } else {
            catalog.query_configurations()?
        };
        if catalog.layouts.is_empty() {
            return Err(invalid("plugin reports no usable audio configurations"));
        }
        Ok(catalog)
    }

    pub(super) fn layouts(&self) -> &[BusLayout] {
        &self.layouts
    }

    #[cfg(test)]
    pub(super) fn test_catalog(layouts: Vec<BusLayout>) -> Self {
        Self {
            plugin: ptr::null(),
            audio_ports: ptr::null(),
            audio_ports_config: ptr::null(),
            audio_ports_config_info: ptr::null(),
            layouts,
        }
    }

    /// Selects the matching native configuration and validates its active ordered bus shape.
    ///
    /// CLAP permits selecting a configuration only while inactive. Callers serialize this method
    /// with processing and invoke it before native activation.
    pub(super) fn select_and_validate(&self, requested: &BusLayout) -> Result<()> {
        let identity = requested
            .id
            .as_deref()
            .map(parse_layout_identity)
            .transpose()?;
        let advertised = self.layouts.iter().any(|layout| {
            layout.id.as_deref() == requested.id.as_deref() && same_topology(layout, requested)
        });
        if !advertised {
            return Err(invalid(
                "requested bus topology is not advertised by this instance",
            ));
        }

        if let Some(config_id) = identity.and_then(|identity| identity.config_id) {
            if self.audio_ports_config.is_null() {
                return Err(invalid("native configuration ID has no config extension"));
            }
            // SAFETY: this extension was returned by the retained inactive plugin instance.
            let config = unsafe { &*self.audio_ports_config };
            let select = config
                .select
                .ok_or_else(|| invalid("config extension has no select callback"))?;
            // SAFETY: caller invokes selection on the serialized main thread while inactive.
            if !unsafe { select(self.plugin, config_id) } {
                return Err(invalid(format!(
                    "plugin rejected audio configuration {config_id}"
                )));
            }
            if !self.audio_ports_config_info.is_null() {
                self.validate_current_config_id(config_id)?;
            }
            let active = self.query_active_layout(Some(config_id))?;
            validate_cached_live_layout(requested, &active)?;
        } else {
            if !self.audio_ports_config.is_null() {
                return Err(invalid("advertised configuration ID is missing"));
            }
            if self.audio_ports.is_null() {
                if requested.inputs.is_empty() && requested.outputs.is_empty() {
                    return Ok(());
                }
                return Err(invalid(
                    "plugin without audio-ports cannot accept audio buses",
                ));
            }
            let active = self.query_active_layout(None)?;
            validate_cached_live_layout(requested, &active)?;
        }
        Ok(())
    }

    fn query_configurations(&mut self) -> Result<Vec<BusLayout>> {
        let original_layout = self
            .audio_ports_config_info
            .is_null()
            .then(|| self.query_active_layout(None))
            .transpose()?;
        let (configs, select) = self.read_configuration_descriptors()?;
        let mut layouts = Vec::new();
        layouts
            .try_reserve_exact(configs.len())
            .map_err(|error| invalid(format!("cannot reserve configuration list: {error}")))?;
        for config in &configs {
            layouts.push(self.query_one_configuration(config, select)?);
        }
        if let Some(original_layout) = original_layout.as_ref() {
            self.restore_initial_configuration(original_layout, &layouts, select)?;
        }
        Ok(layouts)
    }

    fn read_configuration_descriptors(
        &self,
    ) -> Result<(Vec<clap_audio_ports_config>, SelectAudioConfig)> {
        // SAFETY: the config pointer belongs to the same live, inactive plugin instance.
        let extension = unsafe { &*self.audio_ports_config };
        let count_callback = extension
            .count
            .ok_or_else(|| invalid("config extension has no count callback"))?;
        let get_callback = extension
            .get
            .ok_or_else(|| invalid("config extension has no get callback"))?;
        let select_callback = extension
            .select
            .ok_or_else(|| invalid("config extension has no select callback"))?;
        // SAFETY: callback is paired with the retained inactive plugin instance.
        let count = unsafe { count_callback(self.plugin) };
        if count == 0 || count > MAX_AUDIO_CONFIGS {
            return Err(invalid(format!(
                "configuration count {count} is outside 1..={MAX_AUDIO_CONFIGS}"
            )));
        }
        let count_usize = usize::try_from(count)
            .map_err(|_| invalid("configuration count does not fit usize"))?;
        let mut configs = Vec::new();
        configs
            .try_reserve_exact(count_usize)
            .map_err(|error| invalid(format!("cannot reserve configuration list: {error}")))?;
        let mut ids = Vec::new();
        ids.try_reserve_exact(count_usize)
            .map_err(|error| invalid(format!("cannot reserve configuration IDs: {error}")))?;
        self.fill_configuration_descriptors(get_callback, &mut configs, &mut ids, count)?;
        Ok((configs, select_callback))
    }

    fn fill_configuration_descriptors(
        &self,
        get_callback: unsafe extern "C" fn(
            *const clap_plugin,
            u32,
            *mut clap_audio_ports_config,
        ) -> bool,
        configs: &mut Vec<clap_audio_ports_config>,
        ids: &mut Vec<u32>,
        count: u32,
    ) -> Result<()> {
        for index in 0..count {
            let mut config = empty_config();
            // SAFETY: index is bounded by the plugin-reported count and config is writable.
            if !unsafe { get_callback(self.plugin, index, &raw mut config) } {
                return Err(invalid(format!(
                    "config get failed for configuration index {index}"
                )));
            }
            validate_config_name(&config.name)?;
            if ids.contains(&config.id) {
                return Err(invalid(format!(
                    "plugin reports duplicate configuration ID {}",
                    config.id
                )));
            }
            if config.id == CLAP_INVALID_ID {
                return Err(invalid("plugin reports the reserved invalid config ID"));
            }
            ids.push(config.id);
            validate_bus_count(config.input_port_count, "input")?;
            validate_bus_count(config.output_port_count, "output")?;
            configs.push(config);
        }
        Ok(())
    }

    fn query_one_configuration(
        &self,
        config: &clap_audio_ports_config,
        select_callback: SelectAudioConfig,
    ) -> Result<BusLayout> {
        if !self.audio_ports_config_info.is_null() {
            return self.query_config_info_layout(config);
        }
        self.select_native_configuration(select_callback, config.id)?;
        let active = self.query_active_layout(Some(config.id))?;
        validate_config_summary(config, &active)?;
        Ok(active)
    }

    fn select_native_configuration(
        &self,
        select_callback: SelectAudioConfig,
        config_id: u32,
    ) -> Result<()> {
        // SAFETY: caller invokes selection on the serialized main thread while inactive.
        if !unsafe { select_callback(self.plugin, config_id) } {
            return Err(invalid(format!(
                "plugin rejected audio configuration {config_id}"
            )));
        }
        Ok(())
    }

    fn restore_initial_configuration(
        &self,
        original_layout: &BusLayout,
        layouts: &[BusLayout],
        select_callback: SelectAudioConfig,
    ) -> Result<()> {
        let restore_id = restoration_config_id(original_layout, layouts)?;
        self.select_native_configuration(select_callback, restore_id)?;
        if !self.audio_ports_config_info.is_null() {
            self.validate_current_config_id(restore_id)?;
        }
        let active = self.query_active_layout(Some(restore_id))?;
        validate_same_native_ports(original_layout, &active)?;
        Ok(())
    }

    fn query_config_info_layout(&self, config: &clap_audio_ports_config) -> Result<BusLayout> {
        // SAFETY: the info pointer belongs to this live inactive plugin instance.
        let extension = unsafe { &*self.audio_ports_config_info };
        let get_callback = extension
            .get
            .ok_or_else(|| invalid("config-info extension has no get callback"))?;
        let inputs = self.query_config_info_direction(config, true, get_callback)?;
        let outputs = self.query_config_info_direction(config, false, get_callback)?;
        let layout = build_layout(Some(config.id), inputs, outputs)?;
        validate_config_summary(config, &layout)?;
        Ok(layout)
    }

    fn query_config_info_direction(
        &self,
        config: &clap_audio_ports_config,
        is_input: bool,
        get_callback: unsafe extern "C" fn(
            *const clap_plugin,
            clap_sys::id::clap_id,
            u32,
            bool,
            *mut clap_audio_port_info,
        ) -> bool,
    ) -> Result<PortDescriptions> {
        let count = if is_input {
            config.input_port_count
        } else {
            config.output_port_count
        };
        validate_bus_count(count, if is_input { "input" } else { "output" })?;
        let count_usize =
            usize::try_from(count).map_err(|_| invalid("bus count does not fit usize"))?;
        let mut buses = Vec::new();
        buses
            .try_reserve_exact(count_usize)
            .map_err(|error| invalid(format!("cannot reserve bus descriptions: {error}")))?;
        let mut port_ids = Vec::new();
        port_ids
            .try_reserve_exact(count_usize)
            .map_err(|error| invalid(format!("cannot reserve port IDs: {error}")))?;
        for index in 0..count {
            let mut info = empty_port_info();
            // SAFETY: the config ID and port index came from this plugin's bounded config list.
            if !unsafe { get_callback(self.plugin, config.id, index, is_input, &raw mut info) } {
                return Err(invalid(format!(
                    "config-info get failed for {} bus {index}",
                    direction_name(is_input)
                )));
            }
            if port_ids.contains(&info.id) {
                return Err(invalid(format!(
                    "configuration {} reports duplicate {} port ID {}",
                    config.id,
                    direction_name(is_input),
                    info.id
                )));
            }
            if info.id == CLAP_INVALID_ID {
                return Err(invalid(format!(
                    "configuration {} reports the reserved invalid {} port ID",
                    config.id,
                    direction_name(is_input)
                )));
            }
            port_ids.push(info.id);
            buses.push(bus_from_info(&info, is_input, index)?);
        }
        Ok(PortDescriptions {
            buses,
            native_ids: port_ids,
        })
    }

    fn query_active_layout(&self, config_id: Option<u32>) -> Result<BusLayout> {
        let inputs = self.query_active_direction(true)?;
        let outputs = self.query_active_direction(false)?;
        build_layout(config_id, inputs, outputs)
    }

    fn query_active_direction(&self, is_input: bool) -> Result<PortDescriptions> {
        // SAFETY: this extension belongs to the live plugin. The caller is on its main thread.
        let extension = unsafe { &*self.audio_ports };
        let count_callback = extension
            .count
            .ok_or_else(|| invalid("audio-ports extension has no count callback"))?;
        let get_callback = extension
            .get
            .ok_or_else(|| invalid("audio-ports extension has no get callback"))?;
        // SAFETY: callback is paired with this plugin instance.
        let count = unsafe { count_callback(self.plugin, is_input) };
        validate_bus_count(count, if is_input { "input" } else { "output" })?;
        let count_usize =
            usize::try_from(count).map_err(|_| invalid("bus count does not fit usize"))?;
        let mut buses = Vec::new();
        buses
            .try_reserve_exact(count_usize)
            .map_err(|error| invalid(format!("cannot reserve bus descriptions: {error}")))?;
        let mut port_ids = Vec::new();
        port_ids
            .try_reserve_exact(count_usize)
            .map_err(|error| invalid(format!("cannot reserve port IDs: {error}")))?;
        for index in 0..count {
            let mut info = empty_port_info();
            // SAFETY: index is bounded by the count returned for this plugin and direction.
            if !unsafe { get_callback(self.plugin, index, is_input, &raw mut info) } {
                return Err(invalid(format!(
                    "audio-ports get failed for {} bus {index}",
                    direction_name(is_input)
                )));
            }
            if port_ids.contains(&info.id) {
                return Err(invalid(format!(
                    "plugin reports duplicate {} port ID {}",
                    direction_name(is_input),
                    info.id
                )));
            }
            if info.id == CLAP_INVALID_ID {
                return Err(invalid(format!(
                    "plugin reports the reserved invalid {} port ID",
                    direction_name(is_input)
                )));
            }
            port_ids.push(info.id);
            buses.push(bus_from_info(&info, is_input, index)?);
        }
        Ok(PortDescriptions {
            buses,
            native_ids: port_ids,
        })
    }

    fn validate_current_config_id(&self, expected: u32) -> Result<()> {
        // SAFETY: the info pointer belongs to this live inactive plugin instance.
        let extension = unsafe { &*self.audio_ports_config_info };
        let Some(current_config) = extension.current_config else {
            return Ok(());
        };
        // SAFETY: callback is paired with this plugin instance and runs on its main thread.
        let actual = unsafe { current_config(self.plugin) };
        if actual != expected {
            return Err(invalid(format!(
                "plugin selected configuration {actual}, expected {expected}"
            )));
        }
        Ok(())
    }
}

fn validate_bus_count(count: u32, direction: &str) -> Result<()> {
    if count > MAX_AUDIO_BUSES {
        return Err(invalid(format!(
            "{direction} bus count {count} exceeds {MAX_AUDIO_BUSES}"
        )));
    }
    Ok(())
}

fn validate_config_summary(config: &clap_audio_ports_config, layout: &BusLayout) -> Result<()> {
    if usize::try_from(config.input_port_count).ok() != Some(layout.inputs.len())
        || usize::try_from(config.output_port_count).ok() != Some(layout.outputs.len())
    {
        return Err(invalid(format!(
            "configuration {} bus counts disagree with its port descriptions",
            config.id
        )));
    }
    validate_main_bus(
        config.has_main_input,
        config.main_input_channel_count,
        config.main_input_port_type,
        &layout.inputs,
        "input",
    )?;
    validate_main_bus(
        config.has_main_output,
        config.main_output_channel_count,
        config.main_output_port_type,
        &layout.outputs,
        "output",
    )
}

fn validate_main_bus(
    has_main: bool,
    declared_channels: u32,
    declared_port_type: *const c_char,
    buses: &[Bus],
    direction: &str,
) -> Result<()> {
    let mut main_count = 0_u32;
    let mut main_channels = 0_u32;
    for (index, bus) in buses.iter().enumerate() {
        if bus.kind == BusKind::Main {
            main_count += 1;
            main_channels = bus.channels.count();
            if index != 0 {
                return Err(invalid(format!(
                    "main {direction} bus must be the first native bus"
                )));
            }
        }
    }
    if main_count > 1 || has_main != (main_count == 1) {
        return Err(invalid(format!(
            "configuration main {direction} declaration disagrees with its buses"
        )));
    }
    if has_main && declared_channels != main_channels {
        return Err(invalid(format!(
            "configuration main {direction} channel count disagrees with its bus"
        )));
    }
    if has_main {
        let declared_shape = channel_config_from_type(declared_port_type, declared_channels)?;
        let main_shape = buses
            .first()
            .map(|bus| bus.channels)
            .ok_or_else(|| invalid(format!("main {direction} bus is missing")))?;
        if declared_shape != main_shape {
            return Err(invalid(format!(
                "configuration main {direction} channel type disagrees with its bus"
            )));
        }
    }
    if !has_main && declared_channels != 0 {
        return Err(invalid(format!(
            "configuration without a main {direction} reports channels"
        )));
    }
    Ok(())
}

fn build_layout(
    config_id: Option<u32>,
    inputs: PortDescriptions,
    outputs: PortDescriptions,
) -> Result<BusLayout> {
    validate_direction(&inputs, "input")?;
    validate_direction(&outputs, "output")?;
    let mut layout = BusLayout::new();
    layout.id = Some(layout_identity_id(
        config_id,
        &inputs.native_ids,
        &outputs.native_ids,
    )?);
    layout
        .inputs
        .try_reserve_exact(inputs.buses.len())
        .map_err(|error| invalid(format!("cannot reserve input buses: {error}")))?;
    layout
        .outputs
        .try_reserve_exact(outputs.buses.len())
        .map_err(|error| invalid(format!("cannot reserve output buses: {error}")))?;
    layout.inputs.extend(inputs.buses);
    layout.outputs.extend(outputs.buses);
    Ok(layout)
}

fn validate_direction(ports: &PortDescriptions, direction: &str) -> Result<()> {
    if ports.buses.len() != ports.native_ids.len() {
        return Err(invalid(format!(
            "{direction} bus descriptions do not match native port IDs"
        )));
    }
    for (index, port_id) in ports.native_ids.iter().enumerate() {
        if *port_id == CLAP_INVALID_ID {
            return Err(invalid(format!(
                "{direction} port {index} uses the reserved invalid ID"
            )));
        }
        if ports.native_ids[..index].contains(port_id) {
            return Err(invalid(format!(
                "{direction} ports contain duplicate native ID {port_id}"
            )));
        }
    }
    validate_total_channels(&ports.buses, direction)?;
    validate_bus_roles(&ports.buses, direction)
}

fn layout_identity_id(
    config_id: Option<u32>,
    input_ids: &[u32],
    output_ids: &[u32],
) -> Result<String> {
    if config_id == Some(CLAP_INVALID_ID) {
        return Err(invalid(
            "native configuration ID uses the reserved invalid value",
        ));
    }
    let fingerprint = port_ids_fingerprint(input_ids, output_ids);
    let mut value = String::new();
    value
        .try_reserve_exact(80)
        .map_err(|error| invalid(format!("cannot reserve CLAP layout identity: {error}")))?;
    value.push_str("clap:");
    if let Some(config_id) = config_id {
        write!(&mut value, "{config_id}")
            .map_err(|error| invalid(format!("cannot format native config ID: {error}")))?;
    } else {
        value.push_str("current");
    }
    value.push(':');
    for byte in fingerprint {
        write!(&mut value, "{byte:02x}")
            .map_err(|error| invalid(format!("cannot format port fingerprint: {error}")))?;
    }
    Ok(value)
}

fn port_ids_fingerprint(input_ids: &[u32], output_ids: &[u32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"mixrelay-clap-ordered-audio-port-ids-v1\0");
    hash_direction(&mut hasher, b'I', input_ids);
    hash_direction(&mut hasher, b'O', output_ids);
    let digest = hasher.finalize();
    let mut fingerprint = [0; 32];
    fingerprint.copy_from_slice(&digest);
    fingerprint
}

fn hash_direction(hasher: &mut Sha256, direction: u8, ids: &[u32]) {
    hasher.update([direction]);
    let count = u32::try_from(ids.len()).unwrap_or(0);
    hasher.update(count.to_le_bytes());
    for id in ids {
        hasher.update(id.to_le_bytes());
    }
}

fn validate_bus_roles(buses: &[Bus], direction: &str) -> Result<()> {
    let mut main_count = 0;
    for (index, bus) in buses.iter().enumerate() {
        if bus.kind == BusKind::Main {
            main_count += 1;
            if index != 0 {
                return Err(invalid(format!(
                    "main {direction} bus must be the first native bus"
                )));
            }
        }
    }
    if main_count > 1 {
        return Err(invalid(format!("multiple main {direction} buses")));
    }
    Ok(())
}

fn validate_total_channels(buses: &[Bus], direction: &str) -> Result<()> {
    let mut total = 0_u32;
    for bus in buses {
        let channels = bus.channels.count();
        if channels == 0 {
            return Err(invalid(format!("{direction} bus has zero channels")));
        }
        total = total
            .checked_add(channels)
            .ok_or_else(|| invalid(format!("{direction} channel count overflow")))?;
        if total > MAX_AUDIO_CHANNELS {
            return Err(invalid(format!(
                "{direction} total channel count {total} exceeds {MAX_AUDIO_CHANNELS}"
            )));
        }
    }
    Ok(())
}

fn bus_from_info(info: &clap_audio_port_info, is_input: bool, index: u32) -> Result<Bus> {
    if info.channel_count == 0 {
        return Err(invalid(format!(
            "{} bus {index} reports zero channels",
            direction_name(is_input)
        )));
    }
    let flags = info.flags;
    let kind = if flags & CLAP_AUDIO_PORT_IS_MAIN != 0 {
        BusKind::Main
    } else if is_input {
        BusKind::Sidechain
    } else {
        BusKind::Auxiliary
    };
    let mut name = copy_name(&info.name)?;
    if name.is_empty() {
        name = format!("{} {index}", direction_name(is_input));
    }
    Ok(Bus {
        name,
        kind,
        channels: channel_config(info)?,
    })
}

fn channel_config(info: &clap_audio_port_info) -> Result<ChannelConfig> {
    channel_config_from_type(info.port_type, info.channel_count)
}

fn channel_config_from_type(port_type: *const c_char, channel_count: u32) -> Result<ChannelConfig> {
    if channel_count == 0 {
        return Err(invalid("audio port has zero channels"));
    }
    if port_type.is_null() {
        return Ok(ChannelConfig::Discrete(channel_count));
    }
    // SAFETY: CLAP defines port_type as a NUL-terminated identifier valid for the plugin's life.
    let port_type = unsafe { CStr::from_ptr(port_type) };
    if port_type == CLAP_PORT_MONO {
        if channel_count != 1 {
            return Err(invalid("mono port type does not have exactly one channel"));
        }
        Ok(ChannelConfig::Mono)
    } else if port_type == CLAP_PORT_STEREO {
        if channel_count != 2 {
            return Err(invalid(
                "stereo port type does not have exactly two channels",
            ));
        }
        Ok(ChannelConfig::Stereo)
    } else {
        Ok(ChannelConfig::Discrete(channel_count))
    }
}

fn copy_name(name: &[c_char; CLAP_NAME_SIZE]) -> Result<String> {
    let mut bytes = [0_u8; CLAP_NAME_SIZE];
    let length = name
        .iter()
        .enumerate()
        .find_map(|(index, byte)| {
            let byte = byte.to_ne_bytes()[0];
            if byte == 0 {
                Some(index)
            } else {
                bytes[index] = byte;
                None
            }
        })
        .ok_or_else(|| invalid("audio port or config name is not NUL-terminated"))?;
    let value = std::str::from_utf8(&bytes[..length])
        .map_err(|error| invalid(format!("audio port or config name is not UTF-8: {error}")))?;
    let mut owned = String::new();
    owned
        .try_reserve_exact(value.len())
        .map_err(|error| invalid(format!("cannot reserve bus name: {error}")))?;
    owned.push_str(value);
    Ok(owned)
}

fn validate_config_name(name: &[c_char; CLAP_NAME_SIZE]) -> Result<()> {
    copy_name(name).map(|_| ())
}

fn parse_layout_identity(id: &str) -> Result<LayoutIdentity> {
    let raw = id
        .strip_prefix("clap:")
        .ok_or_else(|| invalid("native layout ID is not a CLAP ID"))?;
    let (config_part, fingerprint_part) = raw
        .split_once(':')
        .ok_or_else(|| invalid("native layout ID has no port fingerprint"))?;
    let config_id = if config_part == "current" {
        None
    } else {
        Some(parse_config_id(config_part)?)
    };
    Ok(LayoutIdentity {
        config_id,
        port_ids_fingerprint: parse_fingerprint(fingerprint_part)?,
    })
}

fn parse_config_id(raw: &str) -> Result<u32> {
    let config_id = raw
        .parse::<u32>()
        .map_err(|error| invalid(format!("invalid native configuration ID {raw}: {error}")))?;
    if config_id == CLAP_INVALID_ID {
        return Err(invalid(
            "native configuration ID uses the reserved invalid value",
        ));
    }
    Ok(config_id)
}

fn parse_fingerprint(raw: &str) -> Result<[u8; 32]> {
    if raw.len() != 64 {
        return Err(invalid(
            "native port fingerprint must contain 64 hex digits",
        ));
    }
    let bytes = raw.as_bytes();
    let mut fingerprint = [0; 32];
    for (index, pair) in bytes.chunks_exact(2).enumerate() {
        let high = hex_nibble(pair[0])
            .ok_or_else(|| invalid("native port fingerprint contains non-hex characters"))?;
        let low = hex_nibble(pair[1])
            .ok_or_else(|| invalid("native port fingerprint contains non-hex characters"))?;
        fingerprint[index] = (high << 4) | low;
    }
    Ok(fingerprint)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn restoration_config_id(original: &BusLayout, candidates: &[BusLayout]) -> Result<u32> {
    let original_id = original
        .id
        .as_deref()
        .ok_or_else(|| invalid("initial native layout has no port identity"))?;
    let original_identity = parse_layout_identity(original_id)?;
    for candidate in candidates {
        if !same_topology(original, candidate) {
            continue;
        }
        let candidate_id = candidate
            .id
            .as_deref()
            .ok_or_else(|| invalid("native config layout has no identity"))?;
        let candidate_identity = parse_layout_identity(candidate_id)?;
        if candidate_identity.port_ids_fingerprint == original_identity.port_ids_fingerprint
            && let Some(config_id) = candidate_identity.config_id
        {
            return Ok(config_id);
        }
    }
    Err(invalid(
        "initial native port identity is not an advertised config",
    ))
}

fn validate_cached_live_layout(cached: &BusLayout, live: &BusLayout) -> Result<()> {
    if !same_topology(cached, live) || cached.id != live.id {
        return Err(invalid(
            "live native bus identity differs from the selected layout",
        ));
    }
    Ok(())
}

fn validate_same_native_ports(cached: &BusLayout, live: &BusLayout) -> Result<()> {
    let cached_identity = cached
        .id
        .as_deref()
        .map(parse_layout_identity)
        .transpose()?
        .ok_or_else(|| invalid("cached native layout has no port identity"))?;
    let live_identity = live
        .id
        .as_deref()
        .map(parse_layout_identity)
        .transpose()?
        .ok_or_else(|| invalid("live native layout has no port identity"))?;
    if !same_topology(cached, live)
        || cached_identity.port_ids_fingerprint != live_identity.port_ids_fingerprint
    {
        return Err(invalid(
            "restored native bus identity differs from the initial layout",
        ));
    }
    Ok(())
}

fn same_topology(left: &BusLayout, right: &BusLayout) -> bool {
    same_bus_topology(&left.inputs, &right.inputs)
        && same_bus_topology(&left.outputs, &right.outputs)
}

fn same_bus_topology(left: &[Bus], right: &[Bus]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.kind == right.kind && left.channels == right.channels)
}

fn empty_config() -> clap_audio_ports_config {
    clap_audio_ports_config {
        id: 0,
        name: [0; CLAP_NAME_SIZE],
        input_port_count: 0,
        output_port_count: 0,
        has_main_input: false,
        main_input_channel_count: 0,
        main_input_port_type: ptr::null(),
        has_main_output: false,
        main_output_channel_count: 0,
        main_output_port_type: ptr::null(),
    }
}

fn empty_port_info() -> clap_audio_port_info {
    clap_audio_port_info {
        id: 0,
        name: [0; CLAP_NAME_SIZE],
        flags: 0,
        channel_count: 0,
        port_type: ptr::null(),
        in_place_pair: 0,
    }
}

fn direction_name(is_input: bool) -> &'static str {
    if is_input { "input" } else { "output" }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Other(format!("CLAP audio ports: {}", message.into()))
}

#[cfg(test)]
mod tests;
