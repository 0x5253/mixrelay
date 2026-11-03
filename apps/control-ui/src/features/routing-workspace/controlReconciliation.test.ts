import { expect, it } from 'vitest';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import { acknowledgedNodeControl } from '../../core/domain/graph/acknowledgedNodeControl';
import { controlChanges } from '../../core/domain/graph/controlChanges';
import { rebaseControls, rebaseControlsWithHistory } from './controlReconciliation';
import { runtimeGraphToAudioGraph } from './model';

const before: RuntimeGraphDocument = {
  schemaVersion: 2,
  revision: 3,
  edges: [],
  nodes: [{ id: 'gain', type: 'gain', label: 'Gain', config: { channels: 2, gainDb: 0 } }],
};

it('acknowledged controls only patch the addressed node and preserve topology/configuration', () => {
  const after = acknowledgedNodeControl(before, 'gain', { muted: true, gainDb: -0.12345 }, 4);
  expect(after.revision).toBe(4);
  expect(after.edges).toBe(before.edges);
  expect(after.nodes[0]?.config).toEqual({ channels: 2, gainDb: Math.fround(-0.12345) });
  expect(after.nodes[0]?.control).toEqual({ enabled: true, muted: true });
  expect(before.nodes[0]?.config).toEqual({ channels: 2, gainDb: 0 });
  expect(acknowledgedNodeControl(before, 'deleted', { enabled: false }, 4).nodes[0]).toBe(
    before.nodes[0],
  );
});

function first<T>(items: T[]): T {
  const item = items[0];
  if (!item) throw new Error('Missing fixture item');
  return item;
}

it('rebases only changed controls without moving nodes or discarding local mute', () => {
  const after = structuredClone(before);
  after.revision = 4;
  after.nodes[0] = {
    ...first(after.nodes),
    config: { channels: 2, gainDb: -6 },
    control: { enabled: false, muted: false },
  };
  const changes = controlChanges(before, after);
  expect(changes).toEqual([{ id: 'gain', type: 'gain', value: { enabled: false, gainDb: -6 } }]);
  const model = runtimeGraphToAudioGraph(before);
  first(model.nodes).position = { x: 120, y: 250 };
  const runtime = first(model.nodes).runtime;
  if (!runtime || !changes) throw new Error('fixture');
  runtime.control = { enabled: true, muted: true };
  const result = rebaseControls(model, changes);
  expect(first(result.nodes).position).toEqual({ x: 120, y: 250 });
  expect(first(result.nodes).runtime?.control).toEqual({ enabled: false, muted: true });
  expect(first(result.nodes).runtime?.config).toEqual({ channels: 2, gainDb: -6 });
  expect(runtime.control.enabled).toBe(true);
});

it('structural changes require explicit recovery, including deletion or node type replacement', () => {
  expect(controlChanges(before, { ...before, nodes: [] })).toBeNull();
  const after = structuredClone(before);
  first(after.nodes).type = 'recorder';
  expect(controlChanges(before, after)).toBeNull();
  after.nodes[0] = { ...first(before.nodes), config: { channels: 1, gainDb: 0 } };
  expect(controlChanges(before, after)).toBeNull();
});

it('default controls and object key order do not manufacture structural conflicts', () => {
  const after = {
    ...before,
    revision: 9,
    nodes: [
      {
        ...first(before.nodes),
        config: { gainDb: 0, channels: 2 },
        control: { enabled: true, muted: false },
      },
    ],
  };
  expect(controlChanges(before, after)).toEqual([]);
});

it('restores omitted controls to defaults and ignores nonnumeric gain values', () => {
  const old = {
    ...before,
    nodes: [{ ...first(before.nodes), control: { enabled: false, muted: true } }],
  };
  expect(controlChanges(old, before)).toEqual([
    { id: 'gain', type: 'gain', value: { enabled: true, muted: false } },
  ]);
  const malformed = {
    ...before,
    nodes: [{ ...first(before.nodes), config: { channels: 2, gainDb: 'invalid' } }],
  };
  expect(controlChanges(before, malformed)).toEqual([]);
  const absent = { ...before, nodes: [{ ...first(before.nodes), config: null }] };
  expect(acknowledgedNodeControl(absent, 'gain', { gainDb: -3 }, 4).nodes[0]?.config).toEqual({
    gainDb: -3,
  });
});

it('does not attach runtime controls to UI-only nodes or a replacement processor type', () => {
  const model = runtimeGraphToAudioGraph(before);
  const firstNode = first(model.nodes);
  const uiOnly = { ...firstNode, id: 'visual', runtime: undefined };
  const replacement = {
    ...firstNode,
    runtime: { ...firstNode.runtime, type: 'limiter', config: null },
  };
  const rebased = rebaseControls({ ...model, nodes: [uiOnly, replacement] }, [
    { id: 'gain', type: 'gain', value: { gainDb: -4 } },
  ]);
  expect(rebased.nodes).toEqual([uiOnly, replacement]);
});

it('rebases a mute with missing configuration without inventing gain geometry', () => {
  const model = runtimeGraphToAudioGraph(before);
  const original = first(model.nodes);
  if (!original.runtime) throw new Error('fixture runtime');
  const malformed = {
    ...original,
    appearance: { ...original.appearance, gainControl: undefined },
    runtime: { ...original.runtime, config: null },
  };
  const result = rebaseControls({ ...model, nodes: [malformed] }, [
    { id: 'gain', type: 'gain', value: { gainDb: -4, muted: true } },
  ]);
  expect(first(result.nodes).runtime?.config).toBeNull();
  expect(first(result.nodes).runtime?.control?.muted).toBe(true);
  expect(first(result.nodes).appearance.gainControl).toBeUndefined();
});

it('updates gain configuration without requiring a gain widget or a gain node type', () => {
  const created = runtimeGraphToAudioGraph(before);
  const original = first(created.nodes);
  if (!original.runtime) throw new Error('fixture runtime');
  const virtual = {
    ...original,
    appearance: { ...original.appearance, gainControl: undefined },
    runtime: { ...original.runtime, type: 'application_capture' },
  };
  const result = rebaseControls({ ...created, nodes: [virtual] }, [
    { id: 'gain', type: 'application_capture', value: { gainDb: -8 } },
  ]);
  expect(first(result.nodes).runtime?.config).toMatchObject({ gainDb: -8 });
  expect(first(result.nodes).appearance.detail).toBe(original.appearance.detail);
  expect(first(result.nodes).appearance.gainControl).toBeUndefined();
});

it('keeps unaffected snapshots and nodes shared across a rebased history', () => {
  const after = structuredClone(before);
  after.nodes[0] = { ...first(after.nodes), control: { enabled: false, muted: false } };
  const changes = controlChanges(before, after) ?? [];
  const model = runtimeGraphToAudioGraph(before);
  const moved = { ...model, nodes: model.nodes.map((node) => ({ ...node })) };
  const rebased = new Map();
  const left = rebaseControls(model, changes, rebased);
  // A second snapshot holding the same node object gets the same rebased node.
  const right = rebaseControls({ ...model, revision: 9 }, changes, rebased);
  expect(right.nodes[0]).toBe(left.nodes[0]);
  expect(rebaseControls(moved, changes, rebased).nodes[0]).not.toBe(left.nodes[0]);
  const untouched = { ...model, nodes: [] };
  expect(rebaseControls(untouched, changes)).toBe(untouched);
});

it('rebases the history and the live model with one node cache', () => {
  const after = structuredClone(before);
  after.nodes[0] = { ...first(after.nodes), control: { enabled: false, muted: false } };
  const changes = controlChanges(before, after) ?? [];
  const model = runtimeGraphToAudioGraph(before);
  let historic = model;
  const live = rebaseControlsWithHistory(model, changes, (transform) => {
    historic = transform(historic);
  });
  expect(live.nodes[0]).toBe(historic.nodes[0]);
  expect(live.nodes[0]?.runtime?.control?.enabled).toBe(false);
});
