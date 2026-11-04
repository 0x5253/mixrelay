import type {
  ExternalProcessorDescriptor,
  ProviderBus,
  ProviderNodeConfig,
  ProviderPorts,
} from '../../core/domain/control/processors';
import {
  defaultEffectLabels,
  type EffectKind,
  type EffectParameters,
  effectControls,
} from './builtinEffects';
import { totalBusChannels } from './domain/providerPorts';
import { type GraphPresentation, portLabels } from './graphPresentation';
import {
  AUDIO_PORT_ID,
  channelPorts,
  type NodeBase,
  type ParameterControls,
  type RuntimeAudioNode,
} from './runtimeAudioNode';

export function compressorNode(base: NodeBase, presentation: GraphPresentation): RuntimeAudioNode {
  const config = {
    attackMs: 10,
    bypass: false,
    channels: 2,
    makeupGainDb: 0,
    ratio: 4,
    releaseMs: 120,
    thresholdDb: -18,
    version: 1,
  };
  return processorNode(
    base,
    'compressor',
    presentation.compressor,
    'COMP',
    config,
    compressorControls(config, presentation),
    presentation,
  );
}

/** A built-in effect whose kind, input channels and complete parameters were validated. */
interface ParsedEffect {
  kind: EffectKind;
  parameters: EffectParameters;
  channels: number;
}

export function builtinEffectNode(
  base: NodeBase,
  config: Record<string, unknown>,
  { kind, parameters, channels }: ParsedEffect,
  presentation: GraphPresentation,
): RuntimeAudioNode {
  const labels = presentation.effectLabels ?? defaultEffectLabels;
  const title = labels[kind] ?? defaultEffectLabels[kind];
  const outputs = kind === 'pan' ? 2 : channels;
  const node = processorNode(
    base,
    'effect',
    title,
    'FX',
    config,
    effectControls(kind, parameters, config.bypass === true, labels),
    presentation,
  );
  node.outputPorts = channelPorts(AUDIO_PORT_ID, outputs, title);
  node.appearance.channelCount = outputs;
  node.appearance.meterBanks = [{ channels: outputs, nodeId: base.id, portId: AUDIO_PORT_ID }];
  if (kind === 'equalizer')
    node.appearance.equalizer = {
      bands: [
        {
          id: 'low',
          label: labels.lowDb ?? defaultEffectLabels.lowDb,
          frequencyId: 'lowFrequencyHz',
          gainId: 'lowDb',
          qId: 'lowQ',
        },
        {
          id: 'mid',
          label: labels.midDb ?? defaultEffectLabels.midDb,
          frequencyId: 'midFrequencyHz',
          gainId: 'midDb',
          qId: 'midQ',
        },
        {
          id: 'high',
          label: labels.highDb ?? defaultEffectLabels.highDb,
          frequencyId: 'highFrequencyHz',
          gainId: 'highDb',
          qId: 'highQ',
        },
      ],
    };
  if (kind === 'ducking')
    node.inputPorts.push(
      ...channelPorts('sidechain', 1, labels.sidechain ?? defaultEffectLabels.sidechain),
    );
  return node;
}

export function limiterNode(base: NodeBase, presentation: GraphPresentation): RuntimeAudioNode {
  const config = { bypass: false, ceilingDb: -1, channels: 2, releaseMs: 80, version: 1 };
  return processorNode(
    base,
    'limiter',
    presentation.limiter,
    'LIM',
    config,
    limiterControls(config, presentation),
    presentation,
  );
}

export function externalProcessorNode(
  base: NodeBase,
  processor: ExternalProcessorDescriptor,
  ports: ProviderPorts,
  presentation: GraphPresentation,
): RuntimeAudioNode {
  const parameterValues = Object.fromEntries(
    processor.parameters.map((parameter) => [String(parameter.id), parameter.defaultValue]),
  );
  return {
    ...base,
    appearance: {
      category: `${presentation.processor} · ${processor.format.toUpperCase()}`,
      channelCount: totalBusChannels(ports.outputs),
      icon: processor.format.toUpperCase(),
      hasProcessorEditor: processor.hasEditor,
      meterBanks: ports.outputs.map((bus) => ({
        channels: bus.channels,
        label: providerBusLabel(bus, presentation),
        nodeId: base.id,
        portId: bus.id,
      })),
      optionsLabel: presentation.options,
      parameterControls: externalParameterControls(processor, parameterValues),
      title: processor.displayName,
    },
    inputPorts: providerChannelPorts(ports.inputs, presentation),
    outputPorts: providerChannelPorts(ports.outputs, presentation),
    runtime: {
      config: {
        parameters: { parameterValues, version: 1 },
        ports,
        typeId: processor.typeId,
      } satisfies ProviderNodeConfig,
      type: 'provider',
    },
  };
}

export function providerChannelPorts(
  buses: readonly ProviderBus[],
  presentation: GraphPresentation,
) {
  return buses.flatMap((bus) =>
    channelPorts(bus.id, bus.channels, providerBusLabel(bus, presentation)),
  );
}

export function providerBusLabel(bus: ProviderBus, presentation: GraphPresentation) {
  const kind = presentation.providerBusKinds[bus.kind];
  return bus.name ? `${kind} · ${bus.name}` : kind;
}

export function externalParameterControls(
  processor: ExternalProcessorDescriptor,
  values: Record<string, number>,
): ParameterControls {
  return processor.parameters
    .filter((parameter) => !parameter.isReadOnly)
    .map((parameter) => ({
      id: `external:${parameter.id}`,
      label: parameter.name,
      maximum: 1,
      minimum: 0,
      step: parameter.stepCount > 0 ? 1 / parameter.stepCount : 0.001,
      suffix: '',
      value: values[String(parameter.id)] ?? parameter.defaultValue,
    }));
}

export function processorNode(
  base: NodeBase,
  type: 'compressor' | 'limiter' | 'effect',
  title: string,
  icon: string,
  config: Record<string, unknown>,
  parameterControls: ParameterControls,
  presentation: GraphPresentation,
): RuntimeAudioNode {
  const channels = Number(config.channels);
  const labels = presentation.effectLabels ?? defaultEffectLabels;
  return {
    ...base,
    appearance: {
      category: presentation.processor,
      channelCount: channels,
      detail: processorDetail(type, config),
      icon,
      meterBanks: [{ channels, nodeId: base.id, portId: AUDIO_PORT_ID }],
      optionsLabel: presentation.options,
      parameterControls,
      ...(type === 'compressor' || type === 'limiter'
        ? {
            dynamics: {
              channels,
              nodeId: base.id,
              portId: AUDIO_PORT_ID,
              thresholdId: type === 'compressor' ? 'thresholdDb' : 'ceilingDb',
              inputLabel: labels.dynamicsInput ?? defaultEffectLabels.dynamicsInput,
              outputLabel: labels.dynamicsOutput ?? defaultEffectLabels.dynamicsOutput,
              reductionLabel: labels.gainReduction ?? defaultEffectLabels.gainReduction,
              unavailableLabel:
                labels.dynamicsUnavailable ?? defaultEffectLabels.dynamicsUnavailable,
            },
          }
        : {}),
      title,
    },
    inputPorts: channelPorts(
      AUDIO_PORT_ID,
      channels,
      portLabels(presentation).effectInput.replace('{{name}}', title),
    ),
    outputPorts: channelPorts(
      AUDIO_PORT_ID,
      channels,
      portLabels(presentation).effectOutput.replace('{{name}}', title),
    ),
    runtime: { config, type },
  };
}

export function compressorControls(
  config: Record<string, unknown>,
  presentation: GraphPresentation,
): ParameterControls {
  return [
    dynamicsBypassControl(config, presentation),
    parameterControl('thresholdDb', presentation.threshold, -60, 0, 0.5, 'dB', config),
    parameterControl('ratio', presentation.ratio, 1, 20, 0.5, ':1', config),
    parameterControl('attackMs', presentation.attack, 0.1, 200, 0.1, 'ms', config),
    parameterControl('releaseMs', presentation.release, 5, 2_000, 5, 'ms', config),
    parameterControl('makeupGainDb', presentation.makeupGain, 0, 24, 0.5, 'dB', config),
  ];
}

export function limiterControls(
  config: Record<string, unknown>,
  presentation: GraphPresentation,
): ParameterControls {
  return [
    dynamicsBypassControl(config, presentation),
    parameterControl('ceilingDb', presentation.ceiling, -24, 0, 0.5, 'dB', config),
    parameterControl('releaseMs', presentation.release, 5, 2_000, 5, 'ms', config),
  ];
}

function dynamicsBypassControl(config: Record<string, unknown>, presentation: GraphPresentation) {
  const labels = presentation.effectLabels ?? defaultEffectLabels;
  return {
    id: 'bypass',
    label: labels.bypass ?? defaultEffectLabels.bypass,
    value: Number(config.bypass === true),
    minimum: 0,
    maximum: 1,
    step: 1,
    suffix: '',
    choices: [
      { value: 0, label: labels.processing ?? defaultEffectLabels.processing },
      { value: 1, label: labels.bypassed ?? defaultEffectLabels.bypassed },
    ],
  };
}

function parameterControl(
  id: string,
  label: string,
  minimum: number,
  maximum: number,
  step: number,
  suffix: string,
  config: Record<string, unknown>,
) {
  return {
    id,
    label,
    maximum,
    minimum,
    step,
    suffix,
    value: Number(config[id]),
    precise: true,
    presentation: 'knob' as const,
  };
}

export function processorDetail(type: string, config: Record<string, unknown>): string | undefined {
  if (type === 'compressor') {
    const threshold = Number(config.thresholdDb);
    const ratio = Number(config.ratio);
    return `${threshold.toFixed(1)} dB · ${ratio.toFixed(1)}:1`;
  }
  if (type === 'limiter') {
    const ceiling = Number(config.ceilingDb);
    return `${ceiling.toFixed(1)} dB ceiling`;
  }
  return undefined;
}
