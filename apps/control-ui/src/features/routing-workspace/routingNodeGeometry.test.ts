import { expect, it } from 'vitest';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import { addRoutingNode } from './routingNodeCreation';
import { updateRecorderChannels } from './routingNodeGeometry';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';
import { virtualSpec } from './virtualEndpointFixtures';

const empty: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
const position = { x: 40, y: 60 };
function node(model: AudioGraphModel): AudioGraphNode {
  const first = model.nodes[0];
  if (!first) throw new Error('Expected a materialized fixture node');
  return first;
}

it('allocates a distinct recorder identity and keeps other nodes when resizing its inputs', () => {
  const first = addRoutingNode(runtimeGraphToAudioGraph(empty), { kind: 'recorder' }, position);
  const second = addRoutingNode(first, { kind: 'recorder' }, position);
  expect(new Set(second.nodes.map((item) => item.id)).size).toBe(2);
  expect(second.nodes.map((item) => item.runtime?.type)).toEqual(['recorder', 'recorder']);
  const secondId = second.nodes[1]?.id;
  if (!secondId) throw new Error('second recorder required');
  const changed = updateRecorderChannels(second, secondId, 1);
  expect(changed.nodes[0]).toBe(second.nodes[0]);
  expect(changed.nodes[1]?.inputPorts).toHaveLength(1);
  expect(audioGraphToRuntimeGraph(changed, empty).nodes[1]?.config).toEqual({ channels: 1 });
});

it('removes dangling UI edges when recorder geometry changes', () => {
  const model = addRoutingNode(runtimeGraphToAudioGraph(empty), { kind: 'recorder' }, position);
  const first = node(model);
  const stale = {
    ...model,
    edges: [
      {
        id: 'missing-target',
        sourceNodeId: first.id,
        sourcePortId: 'audio:0',
        targetNodeId: 'removed',
        targetPortId: 'audio:0',
      },
    ],
  };
  expect(updateRecorderChannels(stale, first.id, 1).edges).toEqual([]);
});

it('removes a dangling target even when its source port still exists after geometry edits', () => {
  const first = addRoutingNode(runtimeGraphToAudioGraph(empty), { kind: 'recorder' }, position);
  const created = addRoutingNode(first, { kind: 'virtual_input', spec: virtualSpec }, position);
  const source = created.nodes.find((item) => item.outputPorts.length);
  if (!source?.outputPorts[0]) throw new Error('source fixture');
  const stale = {
    ...created,
    edges: [
      {
        id: 'missing-target',
        sourceNodeId: source.id,
        sourcePortId: source.outputPorts[0].id,
        targetNodeId: 'removed',
        targetPortId: 'audio:0',
      },
    ],
  };
  expect(updateRecorderChannels(stale, node(stale).id, 1).edges).toEqual([]);
});
