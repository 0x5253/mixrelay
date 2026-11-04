import type { ProviderBusKind } from '../../core/domain/control/processors';
import type { EffectLabels } from './builtinEffects';

/** Port group names; `{{name}}` in effect labels is the node title. */
export interface GraphPortLabels {
  application: string;
  deviceInput: string;
  deviceOutput: string;
  effectInput: string;
  effectOutput: string;
  gainInput: string;
  gainOutput: string;
  mixerInput: string;
  mixerOutput: string;
}

const defaultPortLabels: GraphPortLabels = {
  application: 'Application audio',
  deviceInput: 'Device input',
  deviceOutput: 'Device output',
  effectInput: '{{name}} input',
  effectOutput: '{{name}} output',
  gainInput: 'Gain input',
  gainOutput: 'Gain output',
  mixerInput: 'Mixer input',
  mixerOutput: 'Mixer output',
};

export function portLabels(presentation: GraphPresentation) {
  return presentation.ports ?? defaultPortLabels;
}

export interface GraphPresentation {
  effectLabels?: EffectLabels;
  ports?: GraphPortLabels;
  application: string;
  audioInput: string;
  audioOutput: string;
  recorder: string;
  channels: string;
  ceiling: string;
  compressor: string;
  deviceName: string;
  virtualSourcePort: string;
  gain: string;
  mixer: string;
  limiter: string;
  attack: string;
  makeupGain: string;
  options: string;
  outputGain: string;
  processor: string;
  processorLayoutUnavailable: string;
  providerBusKinds: Record<ProviderBusKind, string>;
  ratio: string;
  release: string;
  routing: string;
  sourceGain: string;
  virtualSinkPort: string;
  threshold: string;
  unsupported: string;
  utility: string;
  virtualInput: string;
  virtualOutput: string;
}

export const defaultPresentation: GraphPresentation = {
  application: 'Application',
  audioInput: 'Default input',
  audioOutput: 'Default output',
  recorder: 'Recorder',
  channels: 'Channels',
  ceiling: 'Ceiling',
  compressor: 'Compressor',
  deviceName: 'Device name',
  virtualSourcePort: 'Audio from device',
  gain: 'Gain',
  mixer: 'Mixer',
  limiter: 'Limiter',
  attack: 'Attack',
  makeupGain: 'Makeup',
  options: 'Options',
  outputGain: 'Output gain',
  processor: 'Processor',
  processorLayoutUnavailable: 'Processor layout unavailable',
  providerBusKinds: { main: 'Main', sidechain: 'Sidechain', auxiliary: 'Auxiliary' },
  ratio: 'Ratio',
  release: 'Release',
  routing: 'Routing',
  sourceGain: 'Source gain',
  virtualSinkPort: 'Audio to device',
  threshold: 'Threshold',
  unsupported: 'Unsupported',
  utility: 'Utility',
  virtualInput: 'Virtual input',
  virtualOutput: 'Virtual output',
};
