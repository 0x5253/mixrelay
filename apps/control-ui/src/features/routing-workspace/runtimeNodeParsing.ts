import type { AudioGraphPosition } from '../../core/audio-graph/model';
import type { RuntimeGraphNode } from '../../core/domain/control/graph';
import type { CapturableApplication } from '../../core/domain/control/inventory';
import type {
  ExternalProcessorDescriptor,
  ProviderPorts,
} from '../../core/domain/control/processors';
import { effectParameters, isEffectKind } from './builtinEffects';
import {
  directNumber,
  directString,
  directStringArray,
  nestedNumber,
  nestedString,
  objectValue,
} from './domain/configValues';
import { providerPortsFrom, providerPortsMatch } from './domain/providerPorts';
import { type GraphPresentation, portLabels } from './graphPresentation';
import { applicationNode, audioInputNode, audioOutputNode, recorderNode } from './ioNodes';
import {
  builtinEffectNode,
  compressorControls,
  externalParameterControls,
  externalProcessorNode,
  limiterControls,
  processorNode,
  providerBusLabel,
  providerChannelPorts,
} from './processorNodes';
import {
  AUDIO_PORT_ID,
  channelPorts,
  type NodeBase,
  type NodeRuntime,
  type RuntimeAudioNode,
} from './runtimeAudioNode';
import { virtualEndpointNodes } from './virtualEndpointNodes';

export function runtimeNodeToAudioNodes(
  node: RuntimeGraphNode,
  index: number,
  presentation: GraphPresentation,
  processors: readonly ExternalProcessorDescriptor[],
  reservedIds: ReadonlySet<string>,
): RuntimeAudioNode[] {
  if (node.type === 'virtual_endpoint')
    return virtualEndpointNodes(node, presentation, reservedIds);
  const base = {
    id: node.id,
    position: runtimeNodePosition(node.type, index),
  };
  const runtime = { config: node.config, type: node.type };
  if (node.type === 'recorder') {
    const channels = directNumber(node.config, 'channels');
    if (channels === null || !Number.isInteger(channels) || channels < 1 || channels > 64)
      return [unsupportedRuntimeNode(node, base, presentation)];
    const parsed = recorderNode(base, channels, presentation);
    parsed.runtime = runtime;
    parsed.appearance.title = node.label ?? presentation.recorder;
    return [parsed];
  }
  const knownNode =
    runtimeSourceNode(node, base, runtime, presentation) ??
    runtimeProcessorNode(node, base, runtime, presentation, processors);
  if (knownNode) return [knownNode];
  return [unsupportedRuntimeNode(node, base, presentation)];
}

function runtimeApplicationNode(
  node: RuntimeGraphNode,
  base: NodeBase,
  runtime: NodeRuntime,
  presentation: GraphPresentation,
): RuntimeAudioNode | null {
  const channels = nestedNumber(node.config, 'stream', 'channels');
  const selector = objectValue(node.config)?.selector as
    | CapturableApplication['selector']
    | undefined;
  if (channels === null || !selector) return null;
  const parsed = applicationNode(
    base,
    {
      kind: 'application',
      selector,
      title: node.label ?? node.id,
    },
    presentation,
  );
  parsed.appearance.channelCount = channels;
  parsed.appearance.meterBanks = [{ channels, nodeId: node.id, portId: AUDIO_PORT_ID }];
  parsed.outputPorts = channelPorts(AUDIO_PORT_ID, channels, portLabels(presentation).application);
  parsed.appearance.gainControl = {
    label: presentation.sourceGain,
    valueDb: directNumber(node.config, 'gainDb') ?? 0,
  };
  parsed.runtime = runtime;
  return parsed;
}

function runtimeSourceNode(
  node: RuntimeGraphNode,
  base: NodeBase,
  runtime: NodeRuntime,
  presentation: GraphPresentation,
): RuntimeAudioNode | null {
  if (node.type === 'application_capture')
    return runtimeApplicationNode(node, base, runtime, presentation);
  if (node.type === 'audio_device_input') {
    const channels = directNumber(node.config, 'outputChannels');
    if (channels === null) return null;
    const parsed = audioInputNode(
      base,
      {
        channels,
        kind: 'audio_input',
        title: node.label ?? node.id,
        uid: nestedString(node.config, 'selector', 'uid') ?? '',
      },
      presentation,
    );
    parsed.appearance.gainControl = {
      label: presentation.sourceGain,
      valueDb: directNumber(node.config, 'gainDb') ?? 0,
    };
    parsed.runtime = runtime;
    return parsed;
  }
  if (node.type === 'audio_device_output') {
    const channels = directNumber(node.config, 'inputChannels');
    if (channels === null) return null;
    const parsed = audioOutputNode(
      base,
      {
        channels,
        kind: 'audio_output',
        title: node.label ?? node.id,
        uid: nestedString(node.config, 'selector', 'uid') ?? '',
      },
      presentation,
    );
    parsed.runtime = runtime;
    return parsed;
  }
  return null;
}

function runtimeBuiltinEffectNode(
  node: RuntimeGraphNode,
  base: NodeBase,
  runtime: NodeRuntime,
  presentation: GraphPresentation,
): RuntimeAudioNode | null {
  const config = objectValue(node.config);
  const effect = objectValue(config?.effect);
  if (!config || !isEffectKind(effect?.kind)) return null;
  const parameters = effectParameters(effect.kind, effect.parameters);
  const channels = directNumber(config, 'channels');
  // Bypass is a required field; an editable node would serialize without it.
  if (!parameters || channels === null || typeof config.bypass !== 'boolean') return null;
  const parsed = builtinEffectNode(
    base,
    config,
    { kind: effect.kind, parameters, channels },
    presentation,
  );
  parsed.runtime = runtime;
  parsed.appearance.title = node.label ?? parsed.appearance.title;
  return parsed;
}

function runtimeProcessorNode(
  node: RuntimeGraphNode,
  base: NodeBase,
  runtime: NodeRuntime,
  presentation: GraphPresentation,
  processors: readonly ExternalProcessorDescriptor[],
): RuntimeAudioNode | null {
  if (node.type === 'effect') return runtimeBuiltinEffectNode(node, base, runtime, presentation);
  const dynamics = runtimeDynamicsNode(node, base, runtime, presentation);
  if (dynamics) return dynamics;
  if (node.type === 'provider')
    return runtimeExternalProcessorNode(node, base, runtime, presentation, processors);
  if (node.type === 'gain') {
    const gainDb = directNumber(node.config, 'gainDb');
    const channels = directNumber(node.config, 'channels');
    if (gainDb === null || channels === null) return null;
    return {
      ...base,
      appearance: {
        category: presentation.utility,
        channelCount: channels,
        detail: `${gainDb.toFixed(1)} dB`,
        gainControl: { label: presentation.gain, valueDb: gainDb },
        icon: 'GAIN',
        meterBanks: [
          {
            channels,
            nodeId: node.id,
            portId: AUDIO_PORT_ID,
          },
        ],
        optionsLabel: presentation.options,
        title: node.label ?? node.id,
      },
      inputPorts: channelPorts(AUDIO_PORT_ID, channels, portLabels(presentation).gainInput),
      outputPorts: channelPorts(AUDIO_PORT_ID, channels, portLabels(presentation).gainOutput),
      runtime,
    };
  }
  if (node.type === 'mixer') {
    const inputs = directStringArray(node.config, 'inputs');
    const channels = directNumber(node.config, 'channels');
    if (channels === null) return null;
    return {
      ...base,
      appearance: {
        category: presentation.routing,
        channelCount: channels,
        icon: 'MIX',
        meterBanks: [
          {
            channels,
            nodeId: node.id,
            portId: AUDIO_PORT_ID,
          },
        ],
        title: node.label ?? presentation.mixer,
      },
      inputPorts: inputs.flatMap((id, inputIndex) =>
        channelPorts(id, channels, `${portLabels(presentation).mixerInput} ${inputIndex + 1}`),
      ),
      outputPorts: channelPorts(AUDIO_PORT_ID, channels, portLabels(presentation).mixerOutput),
      runtime,
    };
  }
  return null;
}

function runtimeExternalProcessorNode(
  node: RuntimeGraphNode,
  base: NodeBase,
  runtime: NodeRuntime,
  presentation: GraphPresentation,
  processors: readonly ExternalProcessorDescriptor[],
): RuntimeAudioNode | null {
  const typeId = directString(node.config, 'typeId');
  const descriptor = processors.find((processor) => processor.typeId === typeId);
  const config = objectValue(node.config) ?? {};
  const hasAuthoredPorts = config.ports !== null && config.ports !== undefined;
  const authoredPorts = providerPortsFrom(config.ports);
  if (!descriptor) {
    return {
      ...base,
      runtime: { ...runtime, label: node.label },
      appearance: {
        title: node.label ?? typeId ?? node.id,
        category: presentation.processor,
        icon: 'FX',
        meterBanks: (authoredPorts?.outputs ?? []).map((bus) => ({
          channels: bus.channels,
          label: providerBusLabel(bus, presentation),
          nodeId: base.id,
          portId: bus.id,
        })),
        offline: true,
        detail: presentation.unsupported,
      },
      inputPorts: authoredPorts ? providerChannelPorts(authoredPorts.inputs, presentation) : [],
      outputPorts: authoredPorts ? providerChannelPorts(authoredPorts.outputs, presentation) : [],
    };
  }
  const layout = resolveProcessorLayout(descriptor, authoredPorts, hasAuthoredPorts);
  const parameters = config.parameters;
  const parameterValues = objectValue(objectValue(parameters)?.parameterValues) ?? {};
  const values = Object.fromEntries(
    Object.entries(parameterValues).filter(
      (entry): entry is [string, number] => typeof entry[1] === 'number',
    ),
  );
  const parsed = layout.selectedPorts
    ? externalProcessorNode(base, descriptor, layout.selectedPorts, presentation)
    : unavailableProcessorNode(node, base, descriptor, runtime, presentation);
  parsed.appearance.title = node.label ?? descriptor.displayName;
  parsed.appearance.parameterControls = externalParameterControls(descriptor, values);
  if (layout.unavailable) {
    parsed.appearance.offline = true;
    parsed.appearance.detail = presentation.processorLayoutUnavailable;
  }
  parsed.runtime = layout.materializeSelectedPorts
    ? { ...runtime, config: { ...config, ports: layout.selectedPorts } }
    : runtime;
  return parsed;
}

function resolveProcessorLayout(
  descriptor: ExternalProcessorDescriptor,
  authoredPorts: ProviderPorts | null,
  hasAuthoredPorts: boolean,
) {
  const defaultLayout = providerPortsFrom(
    descriptor.layouts.find((layout) => layout.layoutId === descriptor.defaultLayoutId),
  );
  const selectedPorts = hasAuthoredPorts ? authoredPorts : defaultLayout;
  const catalogLayout = selectedPorts
    ? providerPortsFrom(
        descriptor.layouts.find((layout) => layout.layoutId === selectedPorts.layoutId),
      )
    : null;
  return {
    materializeSelectedPorts: !hasAuthoredPorts && selectedPorts !== null,
    selectedPorts,
    unavailable:
      !selectedPorts || !catalogLayout || !providerPortsMatch(selectedPorts, catalogLayout),
  };
}

function unavailableProcessorNode(
  node: RuntimeGraphNode,
  base: NodeBase,
  descriptor: ExternalProcessorDescriptor,
  runtime: NodeRuntime,
  presentation: GraphPresentation,
): RuntimeAudioNode {
  return {
    ...base,
    appearance: {
      category: `${presentation.processor} · ${descriptor.format.toUpperCase()}`,
      detail: presentation.processorLayoutUnavailable,
      hasProcessorEditor: descriptor.hasEditor,
      icon: descriptor.format.toUpperCase(),
      offline: true,
      title: node.label ?? descriptor.displayName,
    },
    inputPorts: [],
    outputPorts: [],
    runtime,
  };
}

function runtimeDynamicsNode(
  node: RuntimeGraphNode,
  base: NodeBase,
  runtime: NodeRuntime,
  presentation: GraphPresentation,
): RuntimeAudioNode | null {
  if (node.type !== 'compressor' && node.type !== 'limiter') return null;
  const config = objectValue(node.config);
  const compressor = node.type === 'compressor';
  const numericFields = compressor
    ? ['channels', 'thresholdDb', 'ratio', 'attackMs', 'releaseMs', 'makeupGainDb']
    : ['channels', 'ceilingDb', 'releaseMs'];
  if (
    config?.version !== 1 ||
    typeof config.bypass !== 'boolean' ||
    numericFields.some((field) => typeof config[field] !== 'number')
  )
    return null;
  const parsed = processorNode(
    base,
    node.type,
    node.label ?? (compressor ? presentation.compressor : presentation.limiter),
    compressor ? 'COMP' : 'LIM',
    config,
    compressor ? compressorControls(config, presentation) : limiterControls(config, presentation),
    presentation,
  );
  parsed.runtime = runtime;
  return parsed;
}

function unsupportedRuntimeNode(
  node: RuntimeGraphNode,
  base: NodeBase,
  presentation: GraphPresentation,
): RuntimeAudioNode {
  return {
    ...base,
    appearance: {
      category: presentation.unsupported,
      detail: node.type,
      icon: '?',
      title: node.label ?? node.id,
    },
    inputPorts: [],
    outputPorts: [],
    runtime: { config: node.config, type: node.type, label: node.label },
  };
}

function runtimeNodePosition(type: string, index: number): AudioGraphPosition {
  const y = 96 + index * 156;
  if (type === 'application_capture' || type === 'audio_device_input') return { x: 48, y };
  if (type === 'audio_device_output') return { x: 728, y };
  return { x: 388, y };
}
