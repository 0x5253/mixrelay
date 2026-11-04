import type {
  AudioGraphNode,
  AudioGraphPort,
  AudioGraphPosition,
} from '../../core/audio-graph/model';

export const AUDIO_PORT_ID = 'audio';
export const CHANNEL_PORT_SEPARATOR = '::channel:';

export type NodeBase = { id: string; position: AudioGraphPosition };
export type NodeRuntime = NonNullable<AudioGraphNode['runtime']>;
export type ParameterControls = NonNullable<AudioGraphNode['appearance']['parameterControls']>;

// Runtime projection always retains native identity and channel metadata.
export type RuntimeAudioPort = AudioGraphPort &
  Required<Pick<AudioGraphPort, 'channelIndex' | 'runtimePortId'>>;
export interface RuntimeAudioNode extends AudioGraphNode {
  inputPorts: RuntimeAudioPort[];
  outputPorts: RuntimeAudioPort[];
  runtime: NodeRuntime;
}

export function channelPorts(
  runtimePortId: string,
  channels: number,
  label: string,
  boundary?: 'sink' | 'source',
): RuntimeAudioPort[] {
  const count = Math.max(0, Math.min(64, Math.trunc(channels)));
  return Array.from({ length: count }, (_, channelIndex) => ({
    boundary,
    channelIndex,
    id: `${runtimePortId}${CHANNEL_PORT_SEPARATOR}${channelIndex}`,
    label: `${label} ${channelIndex + 1}`,
    runtimePortId,
  }));
}

export function runtimeNodeId(node: AudioGraphNode): string {
  return node.runtime?.binding?.nodeId ?? node.id;
}
