//! Native LV2 port topology, control storage, and bounded audio-plane binding.

use crate::parameters;
use latency::classify_latency_port;
pub(super) use latency::{LatencyPort, latency_snapshot};
use truce_rack_core::{
    ProcessFault,
    buffer::AudioBuffer,
    bus::BusLayout,
    error::{Error, Result},
};

mod latency;

pub(super) const MAX_LV2_AUDIO_CHANNELS: usize = 64;
/// Cached metadata about one LV2 port.
#[derive(Debug, Clone, Copy)]
pub(super) struct PortInfo {
    pub(super) index: u32,
    pub(super) kind: PortKind,
    pub(super) is_input: bool,
    /// Flattened bus/channel slot for native audio ports, computed at load.
    pub(super) audio_plane: Option<usize>,
}

/// Stack-only view of every active bus plane in flattened host order.
pub(super) struct AudioPlanes {
    pub(super) inputs: [*mut f32; MAX_LV2_AUDIO_CHANNELS],
    pub(super) outputs: [*mut f32; MAX_LV2_AUDIO_CHANNELS],
    pub(super) input_count: usize,
    pub(super) output_count: usize,
}

impl AudioPlanes {
    pub(super) fn collect(
        buffer: &mut AudioBuffer<'_, f32>,
        layout: &BusLayout,
        max_block_size: usize,
    ) -> Result<Self> {
        let frames = buffer.num_frames();
        if frames > max_block_size || u32::try_from(frames).is_err() {
            return Err(Error::Process {
                format: crate::FORMAT,
                fault: ProcessFault::InvalidBlockSize,
            });
        }
        if buffer.num_input_buses() != layout.inputs.len()
            || buffer.num_output_buses() != layout.outputs.len()
            || buffer.total_input_channels() != layout.total_input_channels() as usize
            || buffer.total_output_channels() != layout.total_output_channels() as usize
        {
            return Err(Error::Process {
                format: crate::FORMAT,
                fault: ProcessFault::InvalidAudioMapping,
            });
        }
        let mut planes = Self {
            inputs: [std::ptr::null_mut(); MAX_LV2_AUDIO_CHANNELS],
            outputs: [std::ptr::null_mut(); MAX_LV2_AUDIO_CHANNELS],
            input_count: 0,
            output_count: 0,
        };
        for (bus_index, bus) in layout.inputs.iter().enumerate() {
            let channels = buffer.bus_inputs(bus_index);
            if channels.len() != bus.channels.count() as usize {
                return Err(Error::Process {
                    format: crate::FORMAT,
                    fault: ProcessFault::InvalidAudioMapping,
                });
            }
            for channel in channels {
                if channel.len() < frames || planes.input_count >= planes.inputs.len() {
                    return Err(Error::Process {
                        format: crate::FORMAT,
                        fault: ProcessFault::InvalidAudioMapping,
                    });
                }
                planes.inputs[planes.input_count] = channel.as_ptr().cast_mut();
                planes.input_count += 1;
            }
        }
        for (bus_index, bus) in layout.outputs.iter().enumerate() {
            let channels = buffer.bus_outputs(bus_index);
            if channels.len() != bus.channels.count() as usize {
                return Err(Error::Process {
                    format: crate::FORMAT,
                    fault: ProcessFault::InvalidAudioMapping,
                });
            }
            for channel in channels {
                if channel.len() < frames || planes.output_count >= planes.outputs.len() {
                    return Err(Error::Process {
                        format: crate::FORMAT,
                        fault: ProcessFault::InvalidAudioMapping,
                    });
                }
                planes.outputs[planes.output_count] = channel.as_mut_ptr();
                planes.output_count += 1;
            }
        }
        Ok(planes)
    }

    pub(super) fn validate_ports(&self, ports: &[PortInfo]) -> Result<()> {
        for port in ports {
            if matches!(port.kind, PortKind::Audio) {
                let Some(plane) = port.audio_plane else {
                    return Err(Error::Process {
                        format: crate::FORMAT,
                        fault: ProcessFault::InvalidAudioMapping,
                    });
                };
                if (port.is_input && plane >= self.input_count)
                    || (!port.is_input && plane >= self.output_count)
                {
                    return Err(Error::Process {
                        format: crate::FORMAT,
                        fault: ProcessFault::InvalidAudioMapping,
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum PortKind {
    Audio,
    Control { default: f32 },
    AtomSequence,
    Other,
}

/// One load-time classification result; port metadata and connected scalar offsets travel
/// together so activation cannot accidentally pair different plugin layouts.
pub(super) struct ClassifiedPorts {
    pub(super) ports: Vec<PortInfo>,
    pub(super) audio_in_count: usize,
    pub(super) audio_out_count: usize,
    pub(super) control_value_offset: Vec<usize>,
    pub(super) control_values: Vec<f32>,
    pub(super) latency_port: LatencyPort,
}

/// Temporary Lilv class nodes used only while classifying one plugin's ports.
struct PortClassUris {
    audio: *mut lilv_sys::LilvNode,
    control: *mut lilv_sys::LilvNode,
    atom: *mut lilv_sys::LilvNode,
    input: *mut lilv_sys::LilvNode,
    output: *mut lilv_sys::LilvNode,
}

impl PortClassUris {
    /// # Safety
    /// `world` must remain live until this owner is dropped, which `classify_ports` ensures.
    unsafe fn new(world: *mut lilv_sys::LilvWorld) -> Self {
        Self {
            audio: unsafe {
                lilv_sys::lilv_new_uri(world, lilv_sys::LILV_URI_AUDIO_PORT.as_ptr().cast())
            },
            control: unsafe {
                lilv_sys::lilv_new_uri(world, lilv_sys::LILV_URI_CONTROL_PORT.as_ptr().cast())
            },
            atom: unsafe {
                lilv_sys::lilv_new_uri(world, lilv_sys::LILV_URI_ATOM_PORT.as_ptr().cast())
            },
            input: unsafe {
                lilv_sys::lilv_new_uri(world, lilv_sys::LILV_URI_INPUT_PORT.as_ptr().cast())
            },
            output: unsafe {
                lilv_sys::lilv_new_uri(world, lilv_sys::LILV_URI_OUTPUT_PORT.as_ptr().cast())
            },
        }
    }

    /// Classify one native port with the same class nodes used for the whole plugin.
    ///
    /// # Safety
    /// `plugin`, `port`, and every class node must remain owned by the live Lilv world.
    unsafe fn kind(
        &self,
        plugin: *const lilv_sys::LilvPlugin,
        port: *const lilv_sys::LilvPort,
        default: f32,
    ) -> PortKind {
        if unsafe { lilv_sys::lilv_port_is_a(plugin, port, self.audio) } {
            PortKind::Audio
        } else if unsafe { lilv_sys::lilv_port_is_a(plugin, port, self.control) } {
            PortKind::Control {
                default: if default.is_finite() { default } else { 0.0 },
            }
        } else if unsafe { lilv_sys::lilv_port_is_a(plugin, port, self.atom) } {
            // `atom:supports` is not inspected; unexpected MIDI atoms are ignored by the plugin.
            PortKind::AtomSequence
        } else {
            PortKind::Other
        }
    }
}

impl Drop for PortClassUris {
    fn drop(&mut self) {
        // SAFETY: each node was created once by Lilv in `new` and is owned only here.
        unsafe {
            lilv_sys::lilv_node_free(self.audio);
            lilv_sys::lilv_node_free(self.control);
            lilv_sys::lilv_node_free(self.atom);
            lilv_sys::lilv_node_free(self.input);
            lilv_sys::lilv_node_free(self.output);
        }
    }
}

pub(super) unsafe fn classify_ports(
    world: *mut lilv_sys::LilvWorld,
    plugin: *const lilv_sys::LilvPlugin,
) -> ClassifiedPorts {
    let count = unsafe { lilv_sys::lilv_plugin_get_num_ports(plugin) };
    // SAFETY: both pointers refer to the retained world and its selected plugin; the temporary
    // class nodes are released before this function returns.
    let uris = unsafe { PortClassUris::new(world) };

    let mut ports = Vec::with_capacity(count as usize);
    let mut audio_in_count: usize = 0;
    let mut audio_out_count: usize = 0;
    let mut control_value_offset = Vec::with_capacity(count as usize);
    let mut control_values = Vec::new();

    let mut defaults: Vec<f32> = vec![0.0; count as usize];
    unsafe {
        // Pull every control port's declared default in one call.
        let defaults_ptr = defaults.as_mut_ptr();
        lilv_sys::lilv_plugin_get_port_ranges_float(
            plugin,
            std::ptr::null_mut(), // mins (don't care)
            std::ptr::null_mut(), // maxes (don't care)
            defaults_ptr,
        );
    }

    for idx in 0..count {
        let port = unsafe { lilv_sys::lilv_plugin_get_port_by_index(plugin, idx) };
        if port.is_null() {
            ports.push(PortInfo {
                index: idx,
                kind: PortKind::Other,
                is_input: false,
                audio_plane: None,
            });
            control_value_offset.push(usize::MAX);
            continue;
        }
        let is_input = unsafe { lilv_sys::lilv_port_is_a(plugin, port, uris.input) };
        let is_output = unsafe { lilv_sys::lilv_port_is_a(plugin, port, uris.output) };
        let kind = unsafe { uris.kind(plugin, port, defaults[idx as usize]) };
        if matches!(kind, PortKind::Audio) {
            if is_input {
                audio_in_count += 1;
            } else if is_output {
                audio_out_count += 1;
            }
        }
        if let PortKind::Control { default } = kind {
            control_value_offset.push(control_values.len());
            control_values.push(default);
        } else {
            control_value_offset.push(usize::MAX);
        }
        ports.push(PortInfo {
            index: idx,
            kind,
            is_input,
            audio_plane: None,
        });
    }

    // SAFETY: `world` and `plugin` were created together above, and the classified slices are
    // derived from this plugin's live port metadata.
    let latency_port =
        unsafe { classify_latency_port(world, plugin, &ports, &control_value_offset) };

    ClassifiedPorts {
        ports,
        audio_in_count,
        audio_out_count,
        control_value_offset,
        control_values,
        latency_port,
    }
}

/// The connected scalar storage and metadata for exactly one loaded native plugin.
pub(super) struct PortBindings {
    pub(super) ports: Vec<PortInfo>,
    pub(super) control_values: Vec<f32>,
    pub(super) control_value_offset: Vec<usize>,
    pub(super) parameters: parameters::Parameters,
    pub(super) latency_port: LatencyPort,
    pub(super) latency_frames: Option<u32>,
}

impl PortBindings {
    /// Classify one plugin's native topology and construct its matching scalar storage.
    ///
    /// # Safety
    /// `world` must remain alive and own `plugin` through this synchronous metadata query.
    pub(super) unsafe fn load(
        world: *mut lilv_sys::LilvWorld,
        plugin: *const lilv_sys::LilvPlugin,
        path: &std::path::Path,
    ) -> Result<(Self, BusLayout)> {
        // SAFETY: the caller retains this plugin and its owning world through all queries.
        let ClassifiedPorts {
            mut ports,
            audio_in_count,
            audio_out_count,
            control_value_offset,
            control_values,
            latency_port,
        } = unsafe { classify_ports(world, plugin) };
        // SAFETY: these native pointers and newly classified offsets belong to the same plugin.
        let parameters = unsafe {
            parameters::Parameters::collect(world, plugin, &ports, &control_value_offset)
        }?;
        // SAFETY: port groups are inspected against that same retained native metadata.
        let layout = unsafe { crate::port_groups::classify(world, plugin, &mut ports) }.map_err(
            |reason| Error::LoadFailed {
                path: path.to_owned(),
                reason,
            },
        )?;
        debug_assert_eq!(layout.total_input_channels() as usize, audio_in_count);
        debug_assert_eq!(layout.total_output_channels() as usize, audio_out_count);
        Ok((
            Self {
                latency_frames: match latency_port {
                    LatencyPort::NotReported => Some(0),
                    LatencyPort::OutputControl { .. } | LatencyPort::Unknown => None,
                },
                ports,
                control_values,
                control_value_offset,
                parameters,
                latency_port,
            },
            layout,
        ))
    }
}
