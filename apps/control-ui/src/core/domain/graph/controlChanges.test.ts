import { expect, it } from 'vitest';
import type { RuntimeGraphDocument } from '../control/graph';
import { controlChanges } from './controlChanges';

it('reconciles an omitted native gain as the default without changing the graph structure', () => {
  const before: RuntimeGraphDocument = {
    schemaVersion: 2,
    revision: 2,
    edges: [],
    nodes: [{ id: 'gain', type: 'gain', label: null, config: { gainDb: 3 } }],
  };
  const current: RuntimeGraphDocument = {
    ...before,
    revision: 3,
    nodes: [{ id: 'gain', type: 'gain', label: null, config: null }],
  };
  expect(controlChanges(before, current)).toEqual([
    { id: 'gain', type: 'gain', value: { gainDb: 0 } },
  ]);
});

it('reconciles a gain edit from a native input with omitted unity gain', () => {
  const config = { selector: { match: 'persistent_uid', uid: 'device-input' }, outputChannels: 2 };
  const before: RuntimeGraphDocument = {
    schemaVersion: 2,
    revision: 1,
    edges: [],
    nodes: [{ id: 'input', type: 'audio_device_input', label: null, config }],
  };
  const after: RuntimeGraphDocument = {
    ...before,
    revision: 2,
    nodes: before.nodes.map((node) => ({ ...node, config: { ...config, gainDb: -6 } })),
  };
  expect(controlChanges(before, after)).toEqual([
    { id: 'input', type: 'audio_device_input', value: { gainDb: -6 } },
  ]);
});
