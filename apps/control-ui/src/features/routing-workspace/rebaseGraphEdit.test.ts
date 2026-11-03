import { expect, it } from 'vitest';
import type { RuntimeGraphDocument, RuntimeGraphNode } from '../../core/domain/control/graph';
import { rebaseGraphEdit } from '../../core/domain/graph/rebaseGraphEdit';

const node = (id: string, config: unknown = {}): RuntimeGraphNode => ({
  id,
  type: 'effect',
  label: id,
  config,
});
const graph = (nodes: RuntimeGraphNode[]): RuntimeGraphDocument => ({
  schemaVersion: 2,
  revision: 2,
  nodes,
  edges: [],
});
it('keeps concurrent values and nodes while applying only the changed local parameter', () => {
  const before = graph([node('eq', { parameters: { frequency: 1000, gain: 0, q: 0.8 } })]);
  const local = graph([node('eq', { parameters: { frequency: 1000, gain: 6, q: 0.8 } })]);
  const current = graph([
    node('eq', { parameters: { frequency: 2200, gain: -2, q: 2 } }),
    node('remote'),
  ]);
  expect(rebaseGraphEdit(before, local, current)).toEqual(
    graph([node('eq', { parameters: { frequency: 2200, gain: 6, q: 2 } }), node('remote')]),
  );
  expect(current.nodes[0]?.config).toEqual({ parameters: { frequency: 2200, gain: -2, q: 2 } });
});
it('does not resurrect removed nodes or overwrite concurrent replacements or ID collisions', () => {
  const before = graph([node('removed'), node('replaced')]);
  const local = graph([
    node('removed', { mix: 0.5 }),
    node('replaced', { mix: 0.5 }),
    node('collision'),
    node('new'),
  ]);
  const replacement = { ...node('replaced'), type: 'limiter' };
  const collision = node('collision', { remote: true });
  expect(rebaseGraphEdit(before, local, graph([replacement, collision])).nodes).toEqual([
    replacement,
    collision,
    node('new'),
  ]);
});
it('preserves unrelated concurrent fields, applies explicit removals and scalar/array edits', () => {
  const before = graph([
    node('a', { keep: 1, remove: 2, list: [1, 2], nested: { value: 1 } }),
    node('delete'),
  ]);
  const local = graph([node('a', { keep: 1, list: [1, 3], nested: null, added: true })]);
  const current = graph([
    node('a', { keep: 8, remove: 9, list: [2, 3], nested: { value: 3 }, concurrent: 4 }),
    node('delete'),
  ]);
  expect(rebaseGraphEdit(before, local, current).nodes).toEqual([
    node('a', { keep: 8, list: [1, 3], nested: null, added: true, concurrent: 4 }),
  ]);
});
it('preserves equivalent values despite object key order and rejects orphaned new edges', () => {
  const before = graph([node('a', { a: 1, b: [1] }), node('b')]);
  const edge = {
    id: 'wire',
    source: { nodeId: 'a', portId: 'audio' },
    destination: { nodeId: 'b', portId: 'audio' },
    channelMapping: {
      mode: 'explicit' as const,
      routes: [{ sourceChannel: 0, destinationChannel: 0 }],
    },
  };
  const local = { ...graph([node('a', { b: [1], a: 1 }), node('b')]), edges: [edge] };
  const current = graph([node('a', { a: 9, b: [2] })]);
  expect(rebaseGraphEdit(before, local, current)).toEqual(current);
  expect(
    rebaseGraphEdit({ ...before, edges: [edge] }, before, { ...before, edges: [edge] }).edges,
  ).toEqual([]);
});

it('does not retarget an old parameter edit to a replaced plug-in or built-in algorithm', () => {
  const plugin = {
    ...node('plugin', { typeId: 'old', parameters: { value: 0.2 } }),
    type: 'provider',
  };
  const effect = node('effect', { effect: { kind: 'equalizer', parameters: { gain: 0 } } });
  const before = graph([plugin, effect]);
  const local = graph([
    { ...plugin, config: { typeId: 'old', parameters: { value: 0.8 } } },
    node('effect', { effect: { kind: 'equalizer', parameters: { gain: 6 } } }),
  ]);
  const current = graph([
    { ...plugin, config: { typeId: 'new', parameters: { value: 0.1 } } },
    node('effect', { effect: { kind: 'gate', parameters: { threshold: -30 } } }),
  ]);
  expect(rebaseGraphEdit(before, local, current)).toEqual(current);
});

it('applies a parameter edit when configs are absent or change scalar shape', () => {
  const before = graph([node('a', null)]);
  const local = graph([node('a', { mix: 0.5 })]);
  expect(rebaseGraphEdit(before, local, graph([node('a', null)])).nodes).toEqual(local.nodes);
  expect(rebaseGraphEdit(graph([node('a')]), graph([node('a', null)]), before)).toEqual(before);
});

it('rebases explicit connection edits while retaining concurrent routing metadata', () => {
  const nodes = [node('a'), node('b')];
  const edge = {
    id: 'wire',
    source: { nodeId: 'a', portId: 'audio' },
    destination: { nodeId: 'b', portId: 'audio' },
    channelMapping: {
      mode: 'explicit' as const,
      routes: [{ sourceChannel: 0, destinationChannel: 0 }],
    },
  };
  const before = { ...graph(nodes), edges: [edge] };
  const local = {
    ...before,
    edges: [
      {
        ...edge,
        channelMapping: {
          mode: 'explicit' as const,
          routes: [{ sourceChannel: 1, destinationChannel: 0 }],
        },
      },
    ],
  };
  expect(rebaseGraphEdit(before, local, before).edges).toEqual(local.edges);
});

it.each(['native', 'provider', 'algorithm'])(
  'preserves remote %s replacement when the old node was locally deleted',
  (kind) => {
    const old =
      kind === 'provider'
        ? { ...node('same', { typeId: 'old' }), type: 'provider' }
        : node('same', { effect: { kind: 'equalizer' } });
    const replacement =
      kind === 'native'
        ? { ...node('same'), type: 'limiter' }
        : kind === 'provider'
          ? { ...old, config: { typeId: 'new' } }
          : node('same', { effect: { kind: 'reverb' } });
    const remote = graph([replacement]);
    expect(rebaseGraphEdit(graph([old]), graph([]), remote)).toEqual(remote);
    expect(rebaseGraphEdit(graph([old]), graph([]), graph([old])).nodes).toEqual([]);
  },
);
