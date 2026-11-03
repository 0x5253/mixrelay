import type { AudioGraphModel } from '../../audio-graph/model';
import { AudioMeterStore } from '../../audio-meter/AudioMeterStore';
import type { NativeDeviceFormats } from '../../domain/control/nativeFormat';
import type { EndpointControl } from '../../ui/AudioNodeEndpointOptions';
import type { AudioNodeMeterBank } from '../../ui/AudioNodeMeterBanks';
import type { AudioNodeParameterControl } from '../../ui/AudioNodeParameterGrid';
import type { DynamicsMeterContract } from '../../ui/DynamicsMeter';
import type { GraphicEqualizerContract } from '../../ui/GraphicEqualizer';
import type { ParameterFieldControl } from '../../ui/ParameterField';

export const sampleGraph: AudioGraphModel = {
  revision: 1,
  nodes: [
    {
      id: 'source',
      position: { x: 24, y: 48 },
      inputPorts: [],
      outputPorts: [{ id: 'out-1', label: 'Output CH 1', channelIndex: 0 }],
      appearance: { category: 'Source', icon: 'MIC', title: 'Sample microphone', channelCount: 1 },
    },
    {
      id: 'sink',
      position: { x: 320, y: 48 },
      outputPorts: [],
      inputPorts: [{ id: 'in-1', label: 'Input CH 1', channelIndex: 0 }],
      appearance: { category: 'Output', icon: 'IO', title: 'Sample monitor', channelCount: 1 },
    },
  ],
  edges: [
    {
      id: 'sample-edge',
      sourceNodeId: 'source',
      sourcePortId: 'out-1',
      targetNodeId: 'sink',
      targetPortId: 'in-1',
    },
  ],
};
export const gainControl: ParameterFieldControl = {
  id: 'gain',
  label: '게인',
  minimum: -60,
  maximum: 12,
  step: 0.5,
  suffix: 'dB',
  value: 0,
};
const float = { sampleRate: 48000, channels: 2, sampleFormat: 'float32' as const };
const pcm = { ...float, sampleFormat: 'signed24' as const };
export const sampleFormats: NativeDeviceFormats = {
  render: { current: float, supported: [float, pcm] },
  capture: { current: float, supported: [float, pcm] },
};
export const dynamicsContract: DynamicsMeterContract = {
  nodeId: 'sample-compressor',
  portId: 'audio',
  thresholdId: 'thresholdDb',
  inputLabel: '입력',
  outputLabel: '출력',
  reductionLabel: '게인 감쇠',
  unavailableLabel: '계측 미수신',
};
export const thresholdControls: ParameterFieldControl[] = [
  {
    id: 'thresholdDb',
    label: '임계값',
    value: -18,
    minimum: -60,
    maximum: 0,
    step: 0.5,
    suffix: 'dB',
  },
];
export const sampleMeterStore = new AudioMeterStore();
sampleMeterStore.update({
  protocolVersion: 1,
  revision: 1,
  nodes: [
    {
      nodeId: 'sample-compressor',
      portId: 'audio',
      peaks: [0.5, 0.25],
      dynamics: { inputPeaks: [1, 0.5], gainReductionDb: 12, thresholdDb: -18 },
    },
  ],
});
export const equalizerContract: GraphicEqualizerContract = {
  sampleRate: 48000,
  bands: ['low', 'mid', 'high'].map((id, index) => ({
    id,
    label: ['Low', 'Mid', 'High'][index] ?? id,
    frequencyId: `${id}-frequency`,
    gainId: `${id}-gain`,
    qId: `${id}-q`,
  })),
};
export const equalizerControls: ParameterFieldControl[] = ['low', 'mid', 'high'].flatMap(
  (id, index) => [
    {
      id: `${id}-frequency`,
      label: '주파수',
      value: [150, 1000, 6000][index] ?? 1000,
      minimum: 20,
      maximum: 20000,
      step: 1,
      suffix: 'Hz',
      scale: 'logarithmic',
    },
    {
      id: `${id}-gain`,
      label: '게인',
      value: 0,
      minimum: -18,
      maximum: 18,
      step: 0.5,
      suffix: 'dB',
    },
    {
      id: `${id}-q`,
      label: 'Q',
      value: Math.SQRT1_2,
      minimum: 0.1,
      maximum: 10,
      step: 0.001,
      suffix: '',
    },
  ],
);

export const endpointControl: EndpointControl = {
  displayName: 'Studio monitor',
  nameLabel: '장치 이름',
  channelsLabel: '채널',
  canRename: true,
  renderChannels: 2,
  captureChannels: 2,
  allowedRenderChannels: [2, 4, 8],
  allowedCaptureChannels: [2, 4],
  observedRate: { label: '샘플레이트', value: 48000 },
  sampleRateOptions: [44100, 48000, 96000],
  nativeFormats: sampleFormats,
  nativeFormatLabels: { bitDepth: '비트 심도', unknown: '미확인', mixed: '입출력 불일치' },
};
export const meterBanks: AudioNodeMeterBank[] = [
  { channels: 2, nodeId: 'sample-meter', portId: 'main' },
];
export const labelledMeterBanks: AudioNodeMeterBank[] = [
  { channels: 2, nodeId: 'sample-meter', portId: 'main', label: 'Main' },
  { channels: 1, nodeId: 'sample-meter', portId: 'sidechain', label: 'Sidechain' },
];
export const nodeMeterStore = new AudioMeterStore();
nodeMeterStore.update({
  protocolVersion: 1,
  revision: 1,
  nodes: [
    { nodeId: 'sample-meter', portId: 'main', peaks: [0.72, 0.48] },
    { nodeId: 'sample-meter', portId: 'sidechain', peaks: [0.3] },
    {
      nodeId: 'sample-meter',
      portId: 'surround',
      peaks: Array.from({ length: 16 }, (_, index) => ((index % 8) + 1) / 9),
    },
  ],
});
export const knobParameters: AudioNodeParameterControl[] = [
  {
    id: 'cutoff',
    label: '컷오프',
    value: 1200,
    minimum: 20,
    maximum: 20000,
    step: 1,
    suffix: 'Hz',
    scale: 'logarithmic',
    presentation: 'knob',
  },
  {
    id: 'resonance',
    label: '레조넌스',
    value: 0.7,
    minimum: 0.1,
    maximum: 10,
    step: 0.01,
    suffix: '',
    presentation: 'knob',
  },
  {
    id: 'mix',
    label: '믹스',
    value: 100,
    minimum: 0,
    maximum: 100,
    step: 1,
    suffix: '%',
    presentation: 'knob',
  },
];
export const mixedParameters: AudioNodeParameterControl[] = [
  {
    id: 'mode',
    label: '채널 처리',
    value: 0,
    minimum: 0,
    maximum: 1,
    step: 1,
    suffix: '',
    choices: [
      { value: 0, label: '스테레오', description: 'L → L, R → R' },
      { value: 1, label: '모노 합산', description: '(L + R) / 2 → L/R' },
    ],
  },
  { id: 'level', label: '레벨', value: -6, minimum: -60, maximum: 12, step: 0.5, suffix: 'dB' },
  { ...gainControl, id: 'trim', label: '트림', precise: true },
];
