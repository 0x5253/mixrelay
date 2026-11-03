//! Built-in node configuration and port geometry policy.

use crate::model::{
    AUDIO_PORT_ID, CompiledPort, DeviceSelector, FROM_OS_PORT_ID, MixerConfig, NodeConfig,
    NodeSpec, PortDirection, PortId, TO_OS_PORT_ID, VirtualEndpointConfig,
};
use crate::resolution::{CompileContext, NodeResolutionError, NodeResolver, ResolvedNode};
use mixrelay_audio_io::ApplicationCaptureRequest;
use std::collections::BTreeSet;

#[derive(Debug, Default, Clone, Copy)]
pub struct BuiltInNodeResolver;

impl NodeResolver for BuiltInNodeResolver {
    fn resolve(
        &self,
        node: &NodeSpec,
        context: &CompileContext,
    ) -> Result<ResolvedNode, NodeResolutionError> {
        match &node.config {
            NodeConfig::ApplicationCapture(config) => resolve_application_capture(config, context),
            NodeConfig::AudioDeviceInput(config) => {
                validate_device_selector(&config.selector)?;
                validate_channels(config.output_channels, context.limits.max_channels_per_port)?;
                if !config.gain_db.is_finite() {
                    return Err(invalid("gainDb must be finite"));
                }
                Ok(ResolvedNode {
                    inputs: vec![],
                    outputs: vec![audio_port(PortDirection::Output, config.output_channels)],
                })
            }
            NodeConfig::AudioDeviceOutput(config) => {
                validate_device_selector(&config.selector)?;
                validate_channels(config.input_channels, context.limits.max_channels_per_port)?;
                Ok(ResolvedNode {
                    inputs: vec![audio_port(PortDirection::Input, config.input_channels)],
                    outputs: vec![],
                })
            }
            NodeConfig::Recorder { channels } => {
                validate_channels(
                    *channels,
                    context
                        .limits
                        .max_channels_per_port
                        .min(mixrelay_audio_io::formats::MAX_RECORDING_CHANNELS),
                )?;
                Ok(ResolvedNode {
                    inputs: vec![audio_port(PortDirection::Input, *channels)],
                    outputs: vec![],
                })
            }
            NodeConfig::Compressor(config) => {
                validate_channels(config.channels, context.limits.max_channels_per_port)?;
                validate_compressor(config)?;
                Ok(audio_processor(config.channels))
            }
            NodeConfig::Limiter(config) => {
                validate_channels(config.channels, context.limits.max_channels_per_port)?;
                validate_limiter(config)?;
                Ok(audio_processor(config.channels))
            }
            NodeConfig::Effect(config) => resolve_effect(config, context),
            NodeConfig::Mixer(config) => resolve_mixer(config, context),
            NodeConfig::Gain(config) => {
                validate_channels(config.channels, context.limits.max_channels_per_port)?;
                if !config.gain_db.is_finite() {
                    return Err(invalid("gainDb must be finite"));
                }
                Ok(ResolvedNode {
                    inputs: vec![audio_port(PortDirection::Input, config.channels)],
                    outputs: vec![audio_port(PortDirection::Output, config.channels)],
                })
            }
            NodeConfig::VirtualEndpoint(config) => {
                validate_virtual_endpoint_gain(config.gain_db)?;
                resolve_virtual_endpoint(config, context)
            }
            NodeConfig::VirtualEndpointTransport(spec) => resolve_virtual_transport(spec, context),
            NodeConfig::DeviceBus { channels } => {
                validate_channels(*channels, context.limits.max_channels_per_port)?;
                Ok(audio_processor(*channels))
            }
            NodeConfig::Provider(config) => Err(NodeResolutionError::CapabilityUnavailable {
                reason: format!("node provider is unavailable: {}", config.type_id),
            }),
        }
    }
}

fn resolve_effect(
    config: &crate::EffectConfig,
    context: &CompileContext,
) -> Result<ResolvedNode, NodeResolutionError> {
    validate_channels(config.channels, context.limits.max_channels_per_port)?;
    validate_channels(
        config.output_channels(),
        context.limits.max_channels_per_port,
    )?;
    config
        .validate()
        .map_err(|error| invalid(error.to_string()))?;
    let mut resolved = ResolvedNode {
        inputs: vec![audio_port(PortDirection::Input, config.channels)],
        outputs: vec![audio_port(PortDirection::Output, config.output_channels())],
    };
    if matches!(config.effect, crate::Effect::Ducking(_)) {
        resolved.inputs.push(CompiledPort::new(
            PortId::from(crate::SIDECHAIN_PORT_ID),
            PortDirection::Input,
            1,
        ));
    }
    Ok(resolved)
}

/// Validates a virtual-device reference's gain in decibels before DSP lowering.
/// Both direct endpoint nodes and composed references must use this contract;
/// lowering to a generic gain processor must not bypass the endpoint's limits.
///
/// # Errors
/// Returns an invalid-configuration error for non-finite values or values outside
/// the inclusive -60 to +12 dB range.
pub fn validate_virtual_endpoint_gain(gain_db: f32) -> Result<(), NodeResolutionError> {
    validate_parameter("gainDb", gain_db, -60.0, 12.0)
}

fn audio_processor(channels: u16) -> ResolvedNode {
    ResolvedNode {
        inputs: vec![audio_port(PortDirection::Input, channels)],
        outputs: vec![audio_port(PortDirection::Output, channels)],
    }
}

fn validate_compressor(config: &crate::model::CompressorConfig) -> Result<(), NodeResolutionError> {
    if config.version != 1 {
        return Err(invalid("unsupported compressor parameter version"));
    }
    validate_parameter("thresholdDb", config.threshold_db, -60.0, 0.0)?;
    validate_parameter("ratio", config.ratio, 1.0, 20.0)?;
    validate_parameter("attackMs", config.attack_ms, 0.1, 200.0)?;
    validate_parameter("releaseMs", config.release_ms, 5.0, 2_000.0)?;
    validate_parameter("makeupGainDb", config.makeup_gain_db, 0.0, 24.0)
}

fn validate_limiter(config: &crate::model::LimiterConfig) -> Result<(), NodeResolutionError> {
    if config.version != 1 {
        return Err(invalid("unsupported limiter parameter version"));
    }
    validate_parameter("ceilingDb", config.ceiling_db, -24.0, 0.0)?;
    validate_parameter("releaseMs", config.release_ms, 5.0, 2_000.0)
}

fn validate_parameter(
    name: &str,
    value: f32,
    minimum: f32,
    maximum: f32,
) -> Result<(), NodeResolutionError> {
    if !value.is_finite() || !(minimum..=maximum).contains(&value) {
        return Err(invalid(format!(
            "{name} must be finite and between {minimum} and {maximum}"
        )));
    }
    Ok(())
}

fn resolve_application_capture(
    config: &ApplicationCaptureRequest,
    context: &CompileContext,
) -> Result<ResolvedNode, NodeResolutionError> {
    config
        .validate_contract(context.limits.max_channels_per_port)
        .map_err(|error| invalid(error.to_string()))?;
    Ok(ResolvedNode {
        inputs: vec![],
        outputs: vec![audio_port(PortDirection::Output, config.stream.channels)],
    })
}

fn resolve_mixer(
    config: &MixerConfig,
    context: &CompileContext,
) -> Result<ResolvedNode, NodeResolutionError> {
    validate_channels(config.channels, context.limits.max_channels_per_port)?;
    if config.inputs.is_empty() {
        return Err(invalid("a mixer requires at least one input port"));
    }

    let mut unique_ports = BTreeSet::new();
    let mut inputs = Vec::with_capacity(config.inputs.len());
    for port_id in &config.inputs {
        validate_identifier("mixer input port", port_id.as_str())?;
        if !unique_ports.insert(port_id) {
            return Err(invalid(format!("duplicate mixer input port: {port_id}")));
        }
        inputs.push(CompiledPort::new(
            port_id.clone(),
            PortDirection::Input,
            config.channels,
        ));
    }

    Ok(ResolvedNode {
        inputs,
        outputs: vec![audio_port(PortDirection::Output, config.channels)],
    })
}

fn resolve_virtual_endpoint(
    config: &VirtualEndpointConfig,
    context: &CompileContext,
) -> Result<ResolvedNode, NodeResolutionError> {
    let maximum = context.limits.max_channels_per_port;
    config
        .validate_contract(maximum, maximum)
        .map_err(|error| invalid(error.to_string()))?;
    let reference = config.reference;
    if !reference.render && !reference.capture {
        return Err(invalid(
            "virtual endpoint reference must select a source or sink",
        ));
    }
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    if reference.capture {
        let stream = config
            .spec
            .render
            .ok_or_else(|| invalid("virtual endpoint sink requires a native render stream"))?;
        inputs.push(CompiledPort::new(
            PortId::from(TO_OS_PORT_ID),
            PortDirection::Input,
            stream.channels,
        ));
    }
    if reference.render {
        let channels = config
            .spec
            .capture
            .ok_or_else(|| invalid("virtual endpoint source requires a native capture stream"))?
            .channels;
        outputs.push(CompiledPort::new(
            PortId::from(FROM_OS_PORT_ID),
            PortDirection::Output,
            channels,
        ));
    }
    Ok(ResolvedNode { inputs, outputs })
}

fn resolve_virtual_transport(
    spec: &mixrelay_audio_io::VirtualEndpointSpec,
    context: &CompileContext,
) -> Result<ResolvedNode, NodeResolutionError> {
    let maximum = context.limits.max_channels_per_port;
    spec.validate_contract(maximum, maximum)
        .map_err(|error| invalid(error.to_string()))?;
    Ok(ResolvedNode {
        inputs: spec
            .render
            .iter()
            .map(|stream| {
                CompiledPort::new(
                    PortId::from(TO_OS_PORT_ID),
                    PortDirection::Input,
                    stream.channels,
                )
            })
            .collect(),
        outputs: spec
            .capture
            .iter()
            .map(|stream| {
                CompiledPort::new(
                    PortId::from(FROM_OS_PORT_ID),
                    PortDirection::Output,
                    stream.channels,
                )
            })
            .collect(),
    })
}

fn audio_port(direction: PortDirection, channels: u16) -> CompiledPort {
    CompiledPort::new(PortId::from(AUDIO_PORT_ID), direction, channels)
}

fn validate_channels(channels: u16, maximum: u16) -> Result<(), NodeResolutionError> {
    if channels == 0 || channels > maximum {
        return Err(invalid(format!(
            "channel count must be between 1 and {maximum}, found {channels}"
        )));
    }
    Ok(())
}

fn validate_identifier(name: &str, value: &str) -> Result<(), NodeResolutionError> {
    if value.trim().is_empty() {
        return Err(invalid(format!("{name} cannot be empty")));
    }
    Ok(())
}

fn validate_device_selector(selector: &DeviceSelector) -> Result<(), NodeResolutionError> {
    match selector {
        DeviceSelector::PersistentUid { uid } => validate_identifier("audio device UID", uid),
        DeviceSelector::DefaultInput | DeviceSelector::DefaultOutput => Ok(()),
    }
}

fn invalid(reason: impl Into<String>) -> NodeResolutionError {
    NodeResolutionError::InvalidConfiguration {
        reason: reason.into(),
    }
}

pub(crate) fn resolve_dormant_provider(
    node: &NodeSpec,
    context: &CompileContext,
    error: NodeResolutionError,
) -> Result<ResolvedNode, NodeResolutionError> {
    if node.control.enabled || !matches!(error, NodeResolutionError::CapabilityUnavailable { .. }) {
        return Err(error);
    }
    let NodeConfig::Provider(config) = &node.config else {
        return Err(error);
    };
    let Some(ports) = &config.ports else {
        // No native provider and no saved geometry: keep the dormant identity, but
        // never invent connection ports or instantiate an unavailable processor.
        return Ok(ResolvedNode {
            inputs: Vec::new(),
            outputs: Vec::new(),
        });
    };
    ports
        .validate()
        .map_err(|error| invalid(error.to_string()))?;
    let mut inputs = Vec::with_capacity(ports.inputs.len());
    for bus in &ports.inputs {
        validate_channels(bus.channels, context.limits.max_channels_per_port)?;
        inputs.push(CompiledPort::new(
            bus.id.clone(),
            PortDirection::Input,
            bus.channels,
        ));
    }
    let mut outputs = Vec::with_capacity(ports.outputs.len());
    for bus in &ports.outputs {
        validate_channels(bus.channels, context.limits.max_channels_per_port)?;
        outputs.push(CompiledPort::new(
            bus.id.clone(),
            PortDirection::Output,
            bus.channels,
        ));
    }
    Ok(ResolvedNode { inputs, outputs })
}
