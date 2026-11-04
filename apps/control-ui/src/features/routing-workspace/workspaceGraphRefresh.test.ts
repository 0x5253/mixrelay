import { expect, it } from 'vitest';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import { preserveNodeGeometry, sameAuthoredGraph } from './workspaceGraphRefresh';

function node(id: string, x: number, size?: AudioGraphNode['size']): AudioGraphNode {
  return {
    id,
    position: { x, y: 0 },
    size,
    inputPorts: [],
    outputPorts: [],
    appearance: { icon: 'FX', title: id, category: 'Processor' },
  };
}

const graph = (...nodes: AudioGraphNode[]): AudioGraphModel => ({ revision: 2, edges: [], nodes });

it('keeps the position and size of shown nodes and places new nodes where they were built', () => {
  const previous = graph(node('kept', 70, { width: 440, height: 250 }));
  const next = graph(node('kept', 0), node('added', 388));
  expect(preserveNodeGeometry(next, previous).nodes).toEqual([
    node('kept', 70, { width: 440, height: 250 }),
    node('added', 388),
  ]);
  expect(preserveNodeGeometry(next, null).nodes).toEqual(next.nodes);
});

it('compares authored nodes and cables but not revisions', () => {
  const document: RuntimeGraphDocument = {
    schemaVersion: 2,
    revision: 1,
    nodes: [{ id: 'gain', type: 'gain', label: null, config: { gainDb: 0, channels: 2 } }],
    edges: [],
  };
  expect(sameAuthoredGraph(document, { ...document, revision: 9 })).toBe(true);
  expect(sameAuthoredGraph(document, { ...document, nodes: [] })).toBe(false);
  expect(
    sameAuthoredGraph(document, {
      ...document,
      edges: [
        {
          id: 'loop',
          source: { nodeId: 'gain', portId: 'audio' },
          destination: { nodeId: 'gain', portId: 'audio' },
          channelMapping: { mode: 'auto' },
        },
      ],
    }),
  ).toBe(false);
});
