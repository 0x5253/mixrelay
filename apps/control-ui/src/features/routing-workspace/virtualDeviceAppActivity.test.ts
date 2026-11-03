import { assert, expect, it } from 'vitest';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import type { DeviceAppActivity } from '../../core/domain/control/inventory';
import { i18n } from '../../core/i18n';
import { presentVirtualDeviceAppActivity } from './virtualDeviceAppActivity';
import { virtualPolicy, virtualSpec } from './virtualEndpointFixtures';

const source: AudioGraphNode = {
  id: 'source',
  position: { x: 0, y: 0 },
  inputPorts: [],
  outputPorts: [],
  appearance: { title: 'Alias', category: 'Device', icon: 'ADC' },
  runtime: {
    type: 'virtual_endpoint',
    config: { id: virtualSpec.id },
    binding: { nodeId: 'endpoint', side: 'source' },
  },
};
assert(source.runtime);
const [nativeDevice] = virtualPolicy.devices;
assert(nativeDevice);
const sink: AudioGraphNode = {
  ...source,
  id: 'sink',
  runtime: { ...source.runtime, binding: { nodeId: 'endpoint', side: 'sink' } },
};
const other: AudioGraphNode = { ...source, id: 'hardware', runtime: undefined };
const model: AudioGraphModel = { nodes: [source, sink, other], revision: 1, edges: [] };
const settings = {
  ...virtualPolicy,
  devices: [{ ...nativeDevice, nativeDeviceIds: ['native-a', 'native-b', 'absent'] }],
};
const app = (
  bundleId: string,
  processId: number,
  playing = true,
  recording = false,
): DeviceAppActivity => ({
  bundleId,
  processId,
  displayName: bundleId || `Process ${processId}`,
  playing,
  recording,
});
const t = i18n.getFixedT('en');

it('joins only native UIDs, merges processes and devices, ORs both flags, sorts identities and preserves real icons', () => {
  const records = [
    app('z', 1),
    app('a', 2, false, true),
    app('', 9),
    app('a', 3),
    app('b', 4),
    app('c', 5),
    app('d', 6),
    app('idle', 8, false),
  ];
  const result = presentVirtualDeviceAppActivity(
    model,
    settings,
    {
      state: 'observed',
      devices: {
        'native-a': records,
        'native-b': [
          { ...app('a', 7, false, true), iconDataUrl: 'data:real' },
          { ...app('a', 10), iconDataUrl: 'data:other' },
          app('', 9, false, true),
        ],
        [virtualSpec.id]: [app('wrong-endpoint-key', 11)],
      },
    },
    t,
    'en',
  );
  const playing = result.nodes[0]?.appearance.deviceAppActivity;
  assert(playing);
  const recording = result.nodes[1]?.appearance.deviceAppActivity;
  assert(recording);
  expect(playing.apps.map(({ id }) => id)).toEqual([
    'bundle:a',
    'bundle:b',
    'bundle:c',
    'bundle:d',
    'bundle:z',
    'pid:9',
  ]);
  expect(playing.apps[0]?.iconUrl).toBe('data:real');
  expect(playing.overflowLabel).toBe('+3');
  expect(playing.overflowDescription).toContain('d, z, and Process 9');
  expect(playing.summary).toBe(
    `Apps playing to ${virtualSpec.displayName}: a, b, c, d, z, and Process 9`,
  );
  expect(recording.apps.map(({ id }) => id)).toEqual(['bundle:a', 'pid:9']);
  expect(recording.summary).toContain('Apps recording from');
  expect(result.nodes[2]).toBe(other);
  expect(source.appearance.deviceAppActivity).toBeUndefined();
});

it('treats missing known UIDs and explicit arrays as observed empty, never endpoint IDs', () => {
  const result = presentVirtualDeviceAppActivity(
    model,
    settings,
    { state: 'observed', devices: { 'native-a': [], [virtualSpec.id]: [app('wrong', 1)] } },
    t,
    'pseudo',
  );
  expect(result.nodes[0]?.appearance.deviceAppActivity).toMatchObject({
    apps: [],
    label: 'No apps',
    summary: `No apps playing to ${virtualSpec.displayName}`,
  });
  expect(result.nodes[1]?.appearance.deviceAppActivity?.summary).toBe(
    `No apps recording from ${virtualSpec.displayName}`,
  );
});

it('distinguishes loading/failure from empty and clears observations for missing mappings or binding', () => {
  for (const state of ['loading', 'unavailable'] as const) {
    const result = presentVirtualDeviceAppActivity(model, settings, { state }, t, 'en');
    expect(result.nodes[0]?.appearance.deviceAppActivity).toMatchObject({
      apps: [],
      label: state === 'loading' ? 'Checking apps…' : 'App activity unavailable',
    });
  }
  for (const mapping of [
    undefined,
    virtualPolicy,
    { ...settings, devices: [] },
    { ...settings, devices: [{ ...nativeDevice, nativeDeviceIds: [''] }] },
  ]) {
    const result = presentVirtualDeviceAppActivity(
      model,
      mapping,
      { state: 'observed', devices: {} },
      t,
      'en',
    );
    expect(result.nodes[0]?.appearance.deviceAppActivity?.summary).toBe('App activity unavailable');
  }
  const unbound = {
    ...source,
    runtime: { type: 'virtual_endpoint', config: { id: virtualSpec.id }, binding: undefined },
  };
  const result = presentVirtualDeviceAppActivity(
    { ...model, nodes: [unbound] },
    settings,
    { state: 'observed', devices: {} },
    t,
    'en',
  );
  expect(result.nodes[0]?.appearance.deviceAppActivity?.label).toBe('App activity unavailable');
});
