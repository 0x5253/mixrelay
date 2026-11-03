import { expect, it } from 'vitest';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { AudioDeviceDescriptor } from '../../core/domain/control/inventory';
import { i18n } from '../../core/i18n';
import { presentHardwareNodeStatus } from './hardwareNodeStatus';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './model';

const device: AudioDeviceDescriptor = {
  uid: 'explicit-interface',
  displayName: 'Interface',
  inputChannels: 2,
  outputChannels: 2,
  isDefaultInput: false,
  isDefaultOutput: false,
};
const document: RuntimeGraphDocument = {
  schemaVersion: 2,
  revision: 1,
  nodes: [
    {
      id: 'input',
      label: 'Input',
      type: 'audio_device_input',
      config: {
        selector: { match: 'persistent_uid', uid: device.uid },
        outputChannels: 2,
        gainDb: 0,
      },
    },
    {
      id: 'output',
      label: 'Output',
      type: 'audio_device_output',
      config: { selector: { match: 'persistent_uid', uid: device.uid }, inputChannels: 2 },
    },
  ],
  edges: [
    {
      id: 'route',
      source: { nodeId: 'input', portId: 'audio' },
      destination: { nodeId: 'output', portId: 'audio' },
      channelMapping: { mode: 'auto' },
    },
  ],
};

it('marks only missing explicit device directions offline without editing topology or selectors', () => {
  const model = runtimeGraphToAudioGraph(document);
  const missing = presentHardwareNodeStatus(model, [], true, i18n.t);
  expect(missing.nodes.map((node) => node.appearance)).toEqual(
    model.nodes.map((node) => ({
      ...node.appearance,
      status: i18n.t('routing.nodeStatus.offline'),
      statusTone: 'neutral',
      offline: true,
    })),
  );
  expect(missing.edges).toBe(model.edges);
  expect(missing.revision).toBe(model.revision);
  expect(audioGraphToRuntimeGraph(missing, document)).toEqual(
    audioGraphToRuntimeGraph(model, document),
  );
  const returned = presentHardwareNodeStatus(model, [device], true, i18n.t);
  expect(returned.nodes).toEqual(model.nodes);
  const inputOnly = presentHardwareNodeStatus(
    model,
    [{ ...device, outputChannels: 0 }],
    true,
    i18n.t,
  );
  expect(inputOnly.nodes[0]).toBe(model.nodes[0]);
  expect(inputOnly.nodes[1]?.appearance.statusTone).toBe('neutral');
});

it('does not infer missing devices before discovery or substitute the default output', () => {
  const model = runtimeGraphToAudioGraph(document);
  expect(presentHardwareNodeStatus(model, [], false, i18n.t)).toBe(model);
  const differentDefault = { ...device, uid: 'built-in-speaker', isDefaultOutput: true };
  const presented = presentHardwareNodeStatus(model, [differentDefault], true, i18n.t);
  expect(presented.nodes[1]?.appearance.status).toBe(i18n.t('routing.nodeStatus.offline'));
  expect(presented.nodes[1]?.runtime).toBe(model.nodes[1]?.runtime);
});

it('does not label malformed or default selectors as a missing persistent device', () => {
  const model = runtimeGraphToAudioGraph(document);
  const original = model.nodes[0];
  if (!original?.runtime) throw new Error('fixture runtime');
  const configs = [
    null,
    'invalid',
    {},
    { selector: null },
    { selector: 'invalid' },
    { selector: {} },
    { selector: { match: 'default' } },
    { selector: { match: 'persistent_uid' } },
    { selector: { match: 'persistent_uid', uid: 7 } },
  ];
  for (const config of configs) {
    const candidate = { ...original, runtime: { ...original.runtime, config } };
    expect(
      presentHardwareNodeStatus({ ...model, nodes: [candidate] }, [], true, i18n.t).nodes[0],
    ).toBe(candidate);
  }
  const uiOnly = { ...original, runtime: undefined };
  expect(presentHardwareNodeStatus({ ...model, nodes: [uiOnly] }, [], true, i18n.t).nodes[0]).toBe(
    uiOnly,
  );
});
