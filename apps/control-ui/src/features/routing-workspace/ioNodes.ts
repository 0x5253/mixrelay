import { type GraphPresentation, portLabels } from './graphPresentation';
import type { RoutingNodeRequest } from './nodeRequests';
import {
  AUDIO_PORT_ID,
  channelPorts,
  type NodeBase,
  type RuntimeAudioNode,
} from './runtimeAudioNode';

export function applicationNode(
  base: NodeBase,
  request: Extract<RoutingNodeRequest, { kind: 'application' }>,
  presentation: GraphPresentation,
): RuntimeAudioNode {
  return {
    ...base,
    appearance: {
      category: presentation.application,
      channelCount: 2,
      gainControl: { label: presentation.sourceGain, valueDb: 0 },
      icon: 'APP',
      meterBanks: [{ channels: 2, nodeId: base.id, portId: AUDIO_PORT_ID }],
      optionsLabel: presentation.options,
      title: request.title,
    },
    inputPorts: [],
    outputPorts: channelPorts(AUDIO_PORT_ID, 2, portLabels(presentation).application),
    runtime: {
      config: {
        gainDb: 0,
        selector: request.selector,
        sourcePlayback: 'preserve',
        stream: { channelLayout: 'stereo', channels: 2 },
      },
      type: 'application_capture',
    },
  };
}

export function audioInputNode(
  base: NodeBase,
  request: Extract<RoutingNodeRequest, { kind: 'audio_input' }>,
  presentation: GraphPresentation,
): RuntimeAudioNode {
  return {
    ...base,
    appearance: {
      category: presentation.audioInput,
      channelCount: request.channels,
      gainControl: { label: presentation.sourceGain, valueDb: 0 },
      icon: 'ADC',
      meterBanks: [{ channels: request.channels, nodeId: base.id, portId: AUDIO_PORT_ID }],
      optionsLabel: presentation.options,
      title: request.title,
    },
    inputPorts: [],
    outputPorts: channelPorts(
      AUDIO_PORT_ID,
      request.channels,
      portLabels(presentation).deviceInput,
    ),
    runtime: {
      config: {
        gainDb: 0,
        outputChannels: request.channels,
        selector: { match: 'persistent_uid', uid: request.uid },
      },
      type: 'audio_device_input',
    },
  };
}

export function audioOutputNode(
  base: NodeBase,
  request: Extract<RoutingNodeRequest, { kind: 'audio_output' }>,
  presentation: GraphPresentation,
): RuntimeAudioNode {
  return {
    ...base,
    appearance: {
      category: presentation.audioOutput,
      channelCount: request.channels,
      icon: 'DAC',
      meterBanks: [{ channels: request.channels, nodeId: base.id, portId: AUDIO_PORT_ID }],
      title: request.title,
    },
    inputPorts: channelPorts(
      AUDIO_PORT_ID,
      request.channels,
      portLabels(presentation).deviceOutput,
    ),
    outputPorts: [],
    runtime: {
      config: {
        inputChannels: request.channels,
        selector: { match: 'persistent_uid', uid: request.uid },
      },
      type: 'audio_device_output',
    },
  };
}

export function recorderNode(
  base: NodeBase,
  channels: number,
  presentation: GraphPresentation,
): RuntimeAudioNode {
  return {
    ...base,
    appearance: {
      category: presentation.recorder,
      title: presentation.recorder,
      icon: 'REC',
      channelCount: channels,
      meterBanks: [{ channels, nodeId: base.id, portId: AUDIO_PORT_ID }],
    },
    inputPorts: channelPorts(AUDIO_PORT_ID, channels, presentation.recorder),
    outputPorts: [],
    runtime: { type: 'recorder', config: { channels } },
  };
}
