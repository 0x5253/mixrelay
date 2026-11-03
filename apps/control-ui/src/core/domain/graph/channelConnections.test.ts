import { expect, it } from 'vitest';
import { mapChannelConnections, planChannelConnections } from './channelConnections';
import type { GraphEdge, GraphNode } from './topology';

function node(id: string, inputs: number, outputs: number): GraphNode {
  const ports = (prefix: string, count: number) =>
    Array.from({ length: count }, (_, index) => ({ id: `${prefix}${index + 1}` }));
  return { id, inputPorts: ports('in', inputs), outputPorts: ports('out', outputs) };
}

function graph(): { revision: number; nodes: GraphNode[]; edges: GraphEdge[] } {
  return {
    revision: 1,
    nodes: [node('app', 0, 4), node('fx', 2, 2), node('out', 3, 0)],
    edges: [],
  };
}

const request = {
  sourceNodeId: 'app',
  sourcePortIds: ['out1', 'out2'],
  sourceAnchorPortId: 'out1',
  targetNodeId: 'out',
  targetAnchorPortId: 'in2',
};

it('keeps each selected channel offset from the anchor and reports channels past the target', () => {
  expect(mapChannelConnections(graph(), request)).toEqual({
    pairs: [
      { sourceNodeId: 'app', sourcePortId: 'out1', targetNodeId: 'out', targetPortId: 'in2' },
      { sourceNodeId: 'app', sourcePortId: 'out2', targetNodeId: 'out', targetPortId: 'in3' },
    ],
    outOfRange: [],
    invalid: [],
  });
  // A sparse selection keeps its gap; CH 4 would land past the last input.
  const sparse = mapChannelConnections(graph(), {
    ...request,
    sourcePortIds: ['out1', 'out3', 'out4'],
    targetAnchorPortId: 'in1',
  });
  expect(sparse.pairs.map((pair) => [pair.sourcePortId, pair.targetPortId])).toEqual([
    ['out1', 'in1'],
    ['out3', 'in3'],
  ]);
  expect(sparse.outOfRange).toEqual(['out4']);
});

it('treats missing nodes and ports as invalid or unmapped instead of guessing', () => {
  expect(mapChannelConnections(graph(), { ...request, targetNodeId: 'gone' })).toEqual({
    pairs: [],
    outOfRange: [],
    invalid: ['out1', 'out2'],
  });
  const unknownPort = mapChannelConnections(graph(), {
    ...request,
    sourcePortIds: ['out1', 'missing'],
  });
  expect(unknownPort.invalid).toEqual(['missing']);
  expect(
    mapChannelConnections(graph(), { ...request, targetAnchorPortId: 'missing' }).outOfRange,
  ).toEqual(['out1', 'out2']);
});

it('plans against pending edges so duplicates and feedback loops are skipped with a reason', () => {
  const model = graph();
  model.edges = [
    {
      id: 'existing',
      sourceNodeId: 'app',
      sourcePortId: 'out1',
      targetNodeId: 'fx',
      targetPortId: 'in1',
    },
    {
      id: 'loop',
      sourceNodeId: 'fx',
      sourcePortId: 'out1',
      targetNodeId: 'out',
      targetPortId: 'in1',
    },
  ];
  expect(
    planChannelConnections(model, {
      ...request,
      sourcePortIds: ['out1', 'out2', 'out3'],
      targetNodeId: 'fx',
      targetAnchorPortId: 'in1',
    }),
  ).toEqual({
    accepted: [
      { sourceNodeId: 'app', sourcePortId: 'out2', targetNodeId: 'fx', targetPortId: 'in2' },
    ],
    skipped: [
      { sourcePortId: 'out3', reason: 'out-of-range' },
      { sourcePortId: 'out1', targetPortId: 'in1', reason: 'existing' },
    ],
  });
  // With app already feeding fx, routing fx back into app would close a loop.
  const cycle = graph();
  cycle.nodes[0] = node('app', 2, 4);
  cycle.edges = model.edges.slice(0, 1);
  expect(
    planChannelConnections(cycle, {
      sourceNodeId: 'fx',
      sourcePortIds: ['out1'],
      sourceAnchorPortId: 'out1',
      targetNodeId: 'app',
      targetAnchorPortId: 'in1',
    }).skipped,
  ).toEqual([{ sourcePortId: 'out1', targetPortId: 'in1', reason: 'cycle' }]);
});

it('reports a request with nothing to map as invalid without accepting anything', () => {
  expect(
    planChannelConnections(graph(), { ...request, sourceNodeId: 'gone', sourcePortIds: ['x'] }),
  ).toEqual({ accepted: [], skipped: [{ sourcePortId: 'x', reason: 'invalid' }] });
});
