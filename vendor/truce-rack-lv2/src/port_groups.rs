//! Resolve LV2 port-group metadata into one immutable native audio layout.
//! Lilv queries and allocations occur only during load, never in `process`.

use crate::ports::{MAX_LV2_AUDIO_CHANNELS, PortInfo, PortKind};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::ptr;
use truce_rack_core::bus::{Bus, BusKind, BusLayout, ChannelConfig};

const GROUP: &[u8] = b"http://lv2plug.in/ns/ext/port-groups#group\0";
const MAIN_INPUT: &[u8] = b"http://lv2plug.in/ns/ext/port-groups#mainInput\0";
const MAIN_OUTPUT: &[u8] = b"http://lv2plug.in/ns/ext/port-groups#mainOutput\0";
const SIDE_CHAIN_OF: &[u8] = b"http://lv2plug.in/ns/ext/port-groups#sideChainOf\0";
const ELEMENT: &[u8] = b"http://lv2plug.in/ns/ext/port-groups#element\0";
const INPUT_GROUP: &[u8] = b"http://lv2plug.in/ns/ext/port-groups#InputGroup\0";
const OUTPUT_GROUP: &[u8] = b"http://lv2plug.in/ns/ext/port-groups#OutputGroup\0";
const SYMBOL: &[u8] = b"http://lv2plug.in/ns/lv2core#symbol\0";
const DESIGNATION: &[u8] = b"http://lv2plug.in/ns/lv2core#designation\0";
const INDEX: &[u8] = b"http://lv2plug.in/ns/lv2core#index\0";
const RDF_TYPE: &[u8] = b"http://www.w3.org/1999/02/22-rdf-syntax-ns#type\0";
const OUTPUT: &[u8] = b"http://lv2plug.in/ns/lv2core#OutputPort\0";

struct Nodes(*mut lilv_sys::LilvNodes);

impl Drop for Nodes {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this wrapper owns the Lilv collection exactly once.
            unsafe { lilv_sys::lilv_nodes_free(self.0) };
        }
    }
}

struct Uri(*mut lilv_sys::LilvNode);

impl Uri {
    /// # Safety
    /// The Lilv world must remain live while this node is used.
    unsafe fn new(world: *mut lilv_sys::LilvWorld, bytes: &[u8]) -> Result<Self, String> {
        // SAFETY: URI constants are NUL-terminated and the caller retains `world`.
        let node = unsafe { lilv_sys::lilv_new_uri(world, bytes.as_ptr().cast()) };
        if node.is_null() {
            Err("Lilv could not create an LV2 metadata URI".into())
        } else {
            Ok(Self(node))
        }
    }

    /// # Safety
    /// The Lilv world must remain live while this node is used.
    unsafe fn dynamic(world: *mut lilv_sys::LilvWorld, value: &str) -> Result<Self, String> {
        let c = CString::new(value).map_err(|_| "LV2 group URI contains NUL")?;
        // SAFETY: `c` is NUL-terminated and retained for the call; `world` is live.
        let node = unsafe { lilv_sys::lilv_new_uri(world, c.as_ptr()) };
        if node.is_null() {
            Err("Lilv could not create a group URI".into())
        } else {
            Ok(Self(node))
        }
    }
}

impl Drop for Uri {
    fn drop(&mut self) {
        // SAFETY: the node is owned by this wrapper and its Lilv world outlives the wrapper.
        unsafe { lilv_sys::lilv_node_free(self.0) };
    }
}

struct Keys {
    group: Uri,
    main_input: Uri,
    main_output: Uri,
    side_chain_of: Uri,
    element: Uri,
    input_group: Uri,
    output_group: Uri,
    symbol: Uri,
    designation: Uri,
    index: Uri,
    rdf_type: Uri,
    output_port: Uri,
}

impl Keys {
    /// # Safety
    /// `world` must remain live until this key set has dropped.
    unsafe fn new(world: *mut lilv_sys::LilvWorld) -> Result<Self, String> {
        // SAFETY: each constant is a NUL-terminated URI in the live world.
        unsafe {
            Ok(Self {
                group: Uri::new(world, GROUP)?,
                main_input: Uri::new(world, MAIN_INPUT)?,
                main_output: Uri::new(world, MAIN_OUTPUT)?,
                side_chain_of: Uri::new(world, SIDE_CHAIN_OF)?,
                element: Uri::new(world, ELEMENT)?,
                input_group: Uri::new(world, INPUT_GROUP)?,
                output_group: Uri::new(world, OUTPUT_GROUP)?,
                symbol: Uri::new(world, SYMBOL)?,
                designation: Uri::new(world, DESIGNATION)?,
                index: Uri::new(world, INDEX)?,
                rdf_type: Uri::new(world, RDF_TYPE)?,
                output_port: Uri::new(world, OUTPUT)?,
            })
        }
    }
}

/// Consume an owned Lilv collection, preserving cardinality and rejecting wrong RDF value types.
unsafe fn values(nodes: *mut lilv_sys::LilvNodes, want_uri: bool) -> Result<Vec<String>, String> {
    let nodes = Nodes(nodes);
    if nodes.0.is_null() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    // SAFETY: `nodes` is owned and valid until freed below; iterator operations stay in its range.
    unsafe {
        let mut it = lilv_sys::lilv_nodes_begin(nodes.0);
        while !lilv_sys::lilv_nodes_is_end(nodes.0, it) {
            let node = lilv_sys::lilv_nodes_get(nodes.0, it);
            let correct_type = if want_uri {
                lilv_sys::lilv_node_is_uri(node)
            } else {
                lilv_sys::lilv_node_is_string(node)
            };
            let raw = if want_uri {
                lilv_sys::lilv_node_as_uri(node)
            } else {
                lilv_sys::lilv_node_as_string(node)
            };
            if !correct_type || raw.is_null() {
                return Err("LV2 port-group metadata has an invalid RDF value type".into());
            }
            let Ok(value) = CStr::from_ptr(raw).to_str() else {
                return Err("LV2 port-group metadata is not UTF-8".into());
            };
            result.push(value.to_owned());
            it = lilv_sys::lilv_nodes_next(nodes.0, it);
        }
    }
    Ok(result)
}

fn one(values: Vec<String>, property: &str) -> Result<Option<String>, String> {
    if values.len() > 1 {
        return Err(format!("LV2 {property} has multiple values"));
    }
    Ok(values.into_iter().next())
}

/// # Safety
/// `world`, `subject`, and `predicate` must be live Lilv nodes in the same world.
unsafe fn subject_value(
    world: *mut lilv_sys::LilvWorld,
    subject: *const lilv_sys::LilvNode,
    predicate: *const lilv_sys::LilvNode,
    want_uri: bool,
    name: &str,
) -> Result<Option<String>, String> {
    // SAFETY: Lilv owns the queried RDF graph; `values` owns and frees the returned collection.
    one(
        unsafe {
            values(
                lilv_sys::lilv_world_find_nodes(world, subject, predicate, ptr::null()),
                want_uri,
            )?
        },
        name,
    )
}

/// # Safety
/// `world`, `subject`, and `predicate` must be live Lilv nodes in the same world.
unsafe fn subject_index(
    world: *mut lilv_sys::LilvWorld,
    subject: *const lilv_sys::LilvNode,
    predicate: *const lilv_sys::LilvNode,
) -> Result<Option<usize>, String> {
    // SAFETY: the owned collection stays live for this iteration and is freed by `Nodes`.
    let nodes =
        Nodes(unsafe { lilv_sys::lilv_world_find_nodes(world, subject, predicate, ptr::null()) });
    if nodes.0.is_null() {
        return Ok(None);
    }
    let count = unsafe { lilv_sys::lilv_nodes_size(nodes.0) };
    if count != 1 {
        return Err("LV2 group element has multiple indices".into());
    }
    let node = unsafe { lilv_sys::lilv_nodes_get_first(nodes.0) };
    if !unsafe { lilv_sys::lilv_node_is_int(node) } {
        return Err("LV2 group element index is not an integer".into());
    }
    let index = unsafe { lilv_sys::lilv_node_as_int(node) };
    usize::try_from(index)
        .map(Some)
        .map_err(|_| "LV2 group element index is negative".into())
}

/// # Safety
/// The group node and keys belong to `world` and stay live throughout the read.
unsafe fn read_elements(
    world: *mut lilv_sys::LilvWorld,
    group: *const lilv_sys::LilvNode,
    keys: &Keys,
) -> Result<Vec<Element>, String> {
    // SAFETY: Lilv returns an owned collection of resource nodes.
    let nodes = Nodes(unsafe {
        lilv_sys::lilv_world_find_nodes(world, group, keys.element.0, ptr::null())
    });
    if nodes.0.is_null() {
        return Ok(Vec::new());
    }
    let mut elements = Vec::new();
    // SAFETY: the iterator is only used while `nodes` remains owned and live.
    unsafe {
        let mut it = lilv_sys::lilv_nodes_begin(nodes.0);
        while !lilv_sys::lilv_nodes_is_end(nodes.0, it) {
            let node = lilv_sys::lilv_nodes_get(nodes.0, it);
            let designation =
                subject_value(world, node, keys.designation.0, true, "element designation")?
                    .ok_or("LV2 group element lacks designation")?;
            let index = subject_index(world, node, keys.index.0)?;
            elements.push(Element { designation, index });
            it = lilv_sys::lilv_nodes_next(nodes.0, it);
        }
    }
    Ok(elements)
}

/// # Safety
/// The plugin group URI is queried only within its live Lilv world.
unsafe fn read_group(
    world: *mut lilv_sys::LilvWorld,
    uri: &str,
    keys: &Keys,
) -> Result<GroupMeta, String> {
    // SAFETY: this URI and all predicates are live within `world`.
    let node = unsafe { Uri::dynamic(world, uri)? };
    let symbol = unsafe { subject_value(world, node.0, keys.symbol.0, false, "group symbol")? }
        .ok_or_else(|| format!("LV2 group {uri:?} lacks lv2:symbol"))?;
    let side_of =
        unsafe { subject_value(world, node.0, keys.side_chain_of.0, true, "sideChainOf")? };
    let input =
        unsafe { lilv_sys::lilv_world_ask(world, node.0, keys.rdf_type.0, keys.input_group.0) };
    let output =
        unsafe { lilv_sys::lilv_world_ask(world, node.0, keys.rdf_type.0, keys.output_group.0) };
    if input && output {
        return Err(format!(
            "LV2 group {uri:?} declares both input and output types"
        ));
    }
    let direction = if input {
        Some(true)
    } else if output {
        Some(false)
    } else {
        None
    };
    let elements = unsafe { read_elements(world, node.0, keys)? };
    Ok(GroupMeta {
        uri: uri.into(),
        symbol,
        side_of,
        direction,
        elements,
    })
}

struct GroupMeta {
    uri: String,
    symbol: String,
    side_of: Option<String>,
    direction: Option<bool>,
    elements: Vec<Element>,
}

struct PortMetadata {
    symbol: String,
    group_uri: Option<String>,
    designation: Option<String>,
    side_of: Option<String>,
}

/// # Safety
/// `plugin`, `port`, and all keys must remain live for these Lilv queries.
unsafe fn read_port(
    plugin: *const lilv_sys::LilvPlugin,
    port: *const lilv_sys::LilvPort,
    info: &PortInfo,
    keys: &Keys,
) -> Result<PortMetadata, String> {
    // SAFETY: Lilv owns the port symbol; all returned value collections are freed by `values`.
    let symbol_node = unsafe { lilv_sys::lilv_port_get_symbol(plugin, port) };
    if symbol_node.is_null() {
        return Err(format!("LV2 port {} lacks a symbol", info.index));
    }
    let raw_symbol = unsafe { lilv_sys::lilv_node_as_string(symbol_node) };
    if raw_symbol.is_null() {
        return Err(format!("LV2 port {} has an invalid symbol", info.index));
    }
    let symbol = unsafe { CStr::from_ptr(raw_symbol) }
        .to_str()
        .map_err(|_| format!("LV2 port {} symbol is not UTF-8", info.index))?
        .to_owned();
    let group_uri = one(
        unsafe {
            values(
                lilv_sys::lilv_port_get_value(plugin, port, keys.group.0),
                true,
            )?
        },
        "port group",
    )?;
    if !matches!(info.kind, PortKind::Audio) {
        return Ok(PortMetadata {
            symbol,
            group_uri,
            designation: None,
            side_of: None,
        });
    }
    let is_output = unsafe { lilv_sys::lilv_port_is_a(plugin, port, keys.output_port.0) };
    if info.is_input == is_output {
        return Err(format!(
            "LV2 audio port {} has ambiguous direction",
            info.index
        ));
    }
    let designation = one(
        unsafe {
            values(
                lilv_sys::lilv_port_get_value(plugin, port, keys.designation.0),
                true,
            )?
        },
        "audio port designation",
    )?;
    let side_of = one(
        unsafe {
            values(
                lilv_sys::lilv_port_get_value(plugin, port, keys.side_chain_of.0),
                true,
            )?
        },
        "audio port sideChainOf",
    )?;
    Ok(PortMetadata {
        symbol,
        group_uri,
        designation,
        side_of,
    })
}

struct Element {
    designation: String,
    index: Option<usize>,
}

struct Member {
    port: usize,
    symbol: String,
    designation: Option<String>,
}

struct AudioGroup {
    uri: Option<String>,
    name: String,
    side_of: Option<String>,
    members: Vec<Member>,
}

#[derive(Serialize)]
struct MemberIdentity {
    native_index: u32,
    symbol: String,
    designation: Option<String>,
}

#[derive(Serialize)]
struct BusIdentity {
    group_uri: Option<String>,
    group_symbol: String,
    role: &'static str,
    members: Vec<MemberIdentity>,
}

#[derive(Serialize)]
struct LayoutIdentity {
    version: u8,
    inputs: Vec<BusIdentity>,
    outputs: Vec<BusIdentity>,
}

struct DirectionLayout {
    buses: Vec<Bus>,
    identity: Vec<BusIdentity>,
}

fn layout_id(inputs: Vec<BusIdentity>, outputs: Vec<BusIdentity>) -> Result<String, String> {
    let canonical = serde_json::to_vec(&LayoutIdentity {
        version: 1,
        inputs,
        outputs,
    })
    .map_err(|_| "LV2 audio layout identity could not be encoded")?;
    // Fixed-size digest preserves native identity without a URI/symbol length-dependent cap.
    Ok(format!("lv2-pg-v1:{:x}", Sha256::digest(&canonical)))
}

fn add_audio_port(
    groups: &mut Vec<AudioGroup>,
    slot: usize,
    port: PortMetadata,
    meta: Option<&GroupMeta>,
) -> Result<(), String> {
    let side_of = match (meta.and_then(|m| m.side_of.as_ref()), port.side_of.as_ref()) {
        (Some(a), Some(b)) if a != b => return Err("conflicting LV2 sideChainOf metadata".into()),
        (Some(a), _) => Some(a.clone()),
        (_, Some(b)) => Some(b.clone()),
        _ => None,
    };
    let position = groups.iter().position(|g| g.uri == port.group_uri);
    let member = Member {
        port: slot,
        symbol: port.symbol,
        designation: port.designation,
    };
    if let Some(position) = position {
        if groups[position].side_of != side_of {
            return Err("conflicting LV2 sideChainOf among bus members".into());
        }
        groups[position].members.push(member);
    } else {
        groups.push(AudioGroup {
            uri: port.group_uri,
            name: meta.map_or_else(String::new, |m| m.symbol.clone()),
            side_of,
            members: vec![member],
        });
    }
    Ok(())
}

fn order_members(group: &mut AudioGroup, meta: Option<&GroupMeta>) -> Result<(), String> {
    let Some(meta) = meta else { return Ok(()) };
    let mut designations = HashSet::new();
    if meta
        .elements
        .iter()
        .any(|e| !designations.insert(&e.designation))
    {
        return Err("LV2 group repeats an element designation".into());
    }
    if meta.elements.is_empty() || meta.elements.iter().all(|e| e.index.is_none()) {
        // No explicit element order: LV2 raw port order is stable, and designation is optional.
        return Ok(());
    }
    if meta.elements.len() != group.members.len() || meta.elements.iter().any(|e| e.index.is_none())
    {
        return Err("LV2 group has incomplete element indices".into());
    }
    let mut by_designation = HashMap::new();
    let mut seen_indices = HashSet::new();
    for element in &meta.elements {
        let index = element.index.expect("all element indices checked above");
        if index >= group.members.len()
            || !seen_indices.insert(index)
            || by_designation
                .insert(element.designation.as_str(), index)
                .is_some()
        {
            return Err("LV2 group element index or designation is duplicate/out of range".into());
        }
    }
    let mut seen_members = HashSet::new();
    for member in &group.members {
        let designation = member
            .designation
            .as_deref()
            .ok_or("LV2 indexed group member lacks designation")?;
        let index = *by_designation
            .get(designation)
            .ok_or("LV2 indexed group member has unknown designation")?;
        if !seen_members.insert(index) {
            return Err("LV2 indexed group repeats a member designation".into());
        }
    }
    group.members.sort_by_key(|member| {
        let designation = member
            .designation
            .as_deref()
            .expect("every indexed group member designation validated above");
        by_designation[designation]
    });
    Ok(())
}

fn config(count: usize) -> ChannelConfig {
    // A channel count alone does not declare speaker positions (even for two channels).
    ChannelConfig::Discrete(u32::try_from(count).expect("validated LV2 channel cap"))
}

fn place_main_first(groups: &mut Vec<AudioGroup>, main: Option<&str>) -> Result<(), String> {
    let position = if let Some(uri) = main {
        groups
            .iter()
            .position(|group| group.uri.as_deref() == Some(uri))
            .ok_or("LV2 main group does not contain audio ports")?
    } else if groups.len() == 1 {
        // One group, including an ungrouped bucket, has an unambiguous host-default main.
        0
    } else {
        return Err("ambiguous LV2 audio groups without an explicit main group".into());
    };
    let selected = groups.remove(position);
    groups.insert(0, selected);
    Ok(())
}

fn bus_kind(
    group: &AudioGroup,
    index: usize,
    input: bool,
    main_uri: Option<&str>,
) -> Result<BusKind, String> {
    if index == 0 {
        if group.side_of.is_some() {
            return Err("LV2 main group is also declared a sidechain".into());
        }
        return Ok(BusKind::Main);
    }
    if let Some(target) = &group.side_of {
        if !input || main_uri != Some(target.as_str()) {
            return Err("LV2 sideChainOf does not target the input main group".into());
        }
        return Ok(BusKind::Sidechain);
    }
    Ok(BusKind::Auxiliary)
}

fn finish_direction(
    groups: &mut Vec<AudioGroup>,
    main: Option<&str>,
    input: bool,
    ports: &mut [PortInfo],
    metadata: &HashMap<String, GroupMeta>,
) -> Result<DirectionLayout, String> {
    if groups.is_empty() {
        if main.is_some() {
            return Err("LV2 main group is declared without audio ports".into());
        }
        return Ok(DirectionLayout {
            buses: Vec::new(),
            identity: Vec::new(),
        });
    }
    place_main_first(groups, main)?;
    let total_channels: usize = groups.iter().map(|group| group.members.len()).sum();
    if total_channels > MAX_LV2_AUDIO_CHANNELS {
        return Err(format!(
            "LV2 audio channel count exceeds {MAX_LV2_AUDIO_CHANNELS}"
        ));
    }
    let main_uri = groups[0].uri.clone();
    let mut buses = Vec::with_capacity(groups.len());
    let mut identity = Vec::with_capacity(groups.len());
    let mut plane = 0usize;
    for (bus_index, group) in groups.iter_mut().enumerate() {
        order_members(group, group.uri.as_ref().and_then(|uri| metadata.get(uri)))?;
        let kind = bus_kind(group, bus_index, input, main_uri.as_deref())?;
        let name = if group.uri.is_some() {
            group.name.as_str()
        } else if bus_index == 0 {
            if input { "Input" } else { "Output" }
        } else {
            "Ungrouped"
        };
        buses.push(Bus {
            name: name.into(),
            kind,
            channels: config(group.members.len()),
        });
        let members = group
            .members
            .iter()
            .map(|member| {
                ports[member.port].audio_plane = Some(plane);
                plane += 1;
                MemberIdentity {
                    native_index: ports[member.port].index,
                    symbol: member.symbol.clone(),
                    designation: member.designation.clone(),
                }
            })
            .collect();
        identity.push(BusIdentity {
            group_uri: group.uri.clone(),
            group_symbol: group.name.clone(),
            role: match kind {
                BusKind::Main => "main",
                BusKind::Sidechain => "sidechain",
                BusKind::Auxiliary => "auxiliary",
            },
            members,
        });
    }
    debug_assert_eq!(plane, total_channels);
    Ok(DirectionLayout { buses, identity })
}

/// # Safety
/// `world` owns `plugin` and both remain live for every Lilv metadata query.
pub(super) unsafe fn classify(
    world: *mut lilv_sys::LilvWorld,
    plugin: *const lilv_sys::LilvPlugin,
    ports: &mut [PortInfo],
) -> Result<BusLayout, String> {
    // SAFETY: the world owns every queried port, group and metadata URI.
    let keys = unsafe { Keys::new(world)? };
    // SAFETY: Lilv returns owned value collections, consumed by `values`.
    let main_in = one(
        unsafe {
            values(
                lilv_sys::lilv_plugin_get_value(plugin, keys.main_input.0),
                true,
            )?
        },
        "mainInput",
    )?;
    let main_out = one(
        unsafe {
            values(
                lilv_sys::lilv_plugin_get_value(plugin, keys.main_output.0),
                true,
            )?
        },
        "mainOutput",
    )?;
    let mut all_symbols = HashSet::new();
    let mut group_meta: HashMap<String, GroupMeta> = HashMap::new();
    let mut inputs = Vec::<AudioGroup>::new();
    let mut outputs = Vec::<AudioGroup>::new();

    for (slot, info) in ports.iter().enumerate() {
        // SAFETY: `info.index` came from the native plugin's port enumeration.
        let port = unsafe { lilv_sys::lilv_plugin_get_port_by_index(plugin, info.index) };
        if port.is_null() {
            return Err(format!("LV2 port {} is unavailable", info.index));
        }
        // SAFETY: the native port and key nodes belong to the live plugin world.
        let metadata = unsafe { read_port(plugin, port, info, &keys)? };
        if !all_symbols.insert(metadata.symbol.clone()) {
            return Err(format!("duplicate LV2 port symbol {:?}", metadata.symbol));
        }
        if let Some(uri) = metadata.group_uri.as_ref()
            && !group_meta.contains_key(uri)
        {
            // SAFETY: the group URI resolves within the same live world.
            group_meta.insert(uri.clone(), unsafe { read_group(world, uri, &keys)? });
        }
        if !matches!(info.kind, PortKind::Audio) {
            continue;
        }
        let meta = metadata
            .group_uri
            .as_ref()
            .and_then(|uri| group_meta.get(uri));
        if meta
            .and_then(|group| group.direction)
            .is_some_and(|direction| direction != info.is_input)
        {
            return Err(format!(
                "LV2 group for port {} conflicts with audio direction",
                info.index
            ));
        }
        let direction = if info.is_input {
            &mut inputs
        } else {
            &mut outputs
        };
        add_audio_port(direction, slot, metadata, meta)?;
    }
    validate_groups(&group_meta, &mut all_symbols, &inputs, &outputs)?;
    let input = finish_direction(&mut inputs, main_in.as_deref(), true, ports, &group_meta)?;
    let output = finish_direction(&mut outputs, main_out.as_deref(), false, ports, &group_meta)?;
    let id = layout_id(input.identity, output.identity)?;
    let mut layout = BusLayout::new();
    layout.id = Some(id);
    layout.inputs.extend(input.buses);
    layout.outputs.extend(output.buses);
    Ok(layout)
}

fn validate_groups(
    metadata: &HashMap<String, GroupMeta>,
    symbols: &mut HashSet<String>,
    inputs: &[AudioGroup],
    outputs: &[AudioGroup],
) -> Result<(), String> {
    for group in metadata.values() {
        if !symbols.insert(group.symbol.clone()) {
            return Err(format!(
                "duplicate LV2 group/port symbol {:?}",
                group.symbol
            ));
        }
        let in_use = inputs
            .iter()
            .any(|g| g.uri.as_deref() == Some(group.uri.as_str()));
        let out_use = outputs
            .iter()
            .any(|g| g.uri.as_deref() == Some(group.uri.as_str()));
        if in_use && out_use {
            return Err(format!(
                "LV2 group {:?} mixes input and output audio",
                group.uri
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
