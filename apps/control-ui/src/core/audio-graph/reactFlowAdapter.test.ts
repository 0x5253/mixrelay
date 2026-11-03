import { assert, describe, expect, it } from 'vitest';
import { connectGraph, disconnectGraph } from '../domain/graph/topology';
import {
  type AudioGraphModel,
  graphNodeDefaultWidth,
  normalizeAudioGraphNodeSize,
  resizeAudioGraphNode,
} from './model';
import { audioGraphEdgeEndpoints, namedAudioPorts } from './portNames';
import {
  applyAudioNodeLayers,
  reconcileAudioFlowEdges,
  reconcileAudioFlowNodes,
} from './reactFlowAdapter';

function largeGraph(size: number): AudioGraphModel {
  return {
    revision: 1,
    nodes: Array.from({ length: size }, (_, index) => ({
      id: `node-${index}`,
      position: { x: index * 20, y: 0 },
      inputPorts: index === 0 ? [] : [{ id: 'in', label: 'Input' }],
      outputPorts: index === size - 1 ? [] : [{ id: 'out', label: 'Output' }],
      appearance: { category: 'Test', detail: `${index}`, icon: 'T', title: `${index}` },
    })),
    edges: Array.from({ length: size - 1 }, (_, index) => ({
      id: `edge-${index}`,
      sourceNodeId: `node-${index}`,
      sourcePortId: 'out',
      targetNodeId: `node-${index + 1}`,
      targetPortId: 'in',
    })),
  };
}

describe('React Flow incremental adapter', () => {
  it('keeps open options above another selected node and restores layers after closing', () => {
    const model = largeGraph(3);
    const nodes = reconcileAudioFlowNodes([], model);
    const second = nodes[1];
    assert(second);
    nodes[1] = { ...second, selected: true };
    const opened = applyAudioNodeLayers(nodes, 'node-0');
    expect(opened.map((node) => node.zIndex)).toEqual([3, 2, 1]);
    expect(opened[2]).toBe(nodes[2]);
    expect(applyAudioNodeLayers(opened, 'node-1').map((node) => node.zIndex)).toEqual([1, 3, 1]);
    expect(applyAudioNodeLayers(opened, null).map((node) => node.zIndex)).toEqual([1, 2, 1]);
    expect(nodes[0]?.zIndex).toBe(1);
    expect(opened[0]?.data.graphNode).toBe(model.nodes[0]);
  });
  it('restores presentation sizes without changing the audio revision or other nodes', () => {
    const before = largeGraph(2);
    const flow = reconcileAudioFlowNodes([], before);
    const first = flow[0];
    const second = flow[1];
    assert(first && second);
    const after = resizeAudioGraphNode(before, 'node-0', { width: 420, height: 280 });
    const measured = { ...first, height: 444, measured: { width: 420, height: 444 } };
    const resized = reconcileAudioFlowNodes([measured, second], after);
    expect(after.revision).toBe(before.revision);
    expect(resized[0]).toMatchObject({ width: 420, measured: { height: 444 } });
    expect(resized[0]?.height).toBeUndefined();
    expect(resized[0]?.data.graphNode.size).toEqual({ width: 420, height: 280 });
    expect(resized[1]).toBe(flow[1]);
    expect(resizeAudioGraphNode(after, 'node-0', { width: 420, height: 280 })).toBe(after);
    expect(resizeAudioGraphNode(after, 'node-0', { width: NaN, height: 280 })).toBe(after);
    expect(resizeAudioGraphNode(after, 'node-0', { width: 420, height: NaN })).toBe(after);
    expect(resizeAudioGraphNode(after, 'removed-node', { width: 420, height: 280 })).toBe(after);
    const bounded = resizeAudioGraphNode(after, 'node-0', { width: 1, height: 5000 });
    expect(bounded.nodes[0]?.size).toEqual({ width: 180, height: 4096 });
  });

  it('preserves every unaffected adapter during an edge disconnect', () => {
    const before = largeGraph(1_024);
    const nodes = reconcileAudioFlowNodes([], before);
    const edges = reconcileAudioFlowEdges([], before);
    expect(edges[0]?.type).toBe('default');
    expect(edges[0]?.zIndex).toBe(0);
    expect(nodes[0]?.zIndex).toBe(1);
    expect(reconcileAudioFlowEdges(edges, before)).toBe(edges);
    const after = disconnectGraph(before, ['edge-511']);
    const nextNodes = reconcileAudioFlowNodes(nodes, after);
    const nextEdges = reconcileAudioFlowEdges(edges, after);

    expect(nextNodes).toBe(nodes);
    expect(nextNodes.every((node, index) => node === nodes[index])).toBe(true);
    expect(nextEdges).toHaveLength(1_022);
    expect(nextEdges.every((edge) => edges.includes(edge))).toBe(true);
  });

  it('allocates only the newly connected edge and keeps transient selection state', () => {
    const before: AudioGraphModel = {
      ...largeGraph(3),
      edges: [],
    };
    const nodes = reconcileAudioFlowNodes([], before);
    const selectedNodes = nodes.map((node, index) =>
      index === 1 ? { ...node, selected: true } : node,
    );
    const after = connectGraph(before, {
      sourceNodeId: 'node-0',
      sourcePortId: 'out',
      targetNodeId: 'node-1',
      targetPortId: 'in',
    });
    const nextNodes = reconcileAudioFlowNodes(selectedNodes, after);
    const nextEdges = reconcileAudioFlowEdges([], after);

    expect(nextNodes).toBe(selectedNodes);
    expect(nextNodes[1]).toBe(selectedNodes[1]);
    expect(nextNodes[1]?.selected).toBe(true);
    expect(nextEdges).toHaveLength(1);
  });

  it('replaces only the adapter whose domain object changed', () => {
    const before = largeGraph(4);
    const nodes = reconcileAudioFlowNodes([], before);
    const changed: AudioGraphModel = {
      ...before,
      nodes: before.nodes.map((node, index) =>
        index === 2 ? { ...node, position: { x: 999, y: 20 } } : node,
      ),
    };
    const nextNodes = reconcileAudioFlowNodes(nodes, changed);

    expect(nextNodes[0]).toBe(nodes[0]);
    expect(nextNodes[1]).toBe(nodes[1]);
    expect(nextNodes[2]).not.toBe(nodes[2]);
    expect(nextNodes[3]).toBe(nodes[3]);
  });
});

it('normalizes a narrow saved EQ size consistently for resize and React Flow geometry', () => {
  const original = largeGraph(1);
  const node = original.nodes[0];
  assert(node);
  node.appearance.equalizer = { bands: [] };
  node.size = { width: 228, height: 160 };
  const flow = reconcileAudioFlowNodes([], original);
  expect(flow[0]?.width).toBe(320);
  expect(node.size).toEqual({ width: 228, height: 160 });
  const resized = resizeAudioGraphNode(original, node.id, { width: 100, height: 200 });
  expect(resized.nodes[0]?.size).toEqual({ width: 320, height: 200 });
  expect(resized.revision).toBe(original.revision);
  expect(normalizeAudioGraphNodeSize(node, undefined)).toBeUndefined();
  const roomy = { width: 480, height: 210 };
  expect(normalizeAudioGraphNodeSize(node, roomy)).toBe(roomy);
  expect(normalizeAudioGraphNodeSize(node, node.size)).toEqual({ width: 320, height: 160 });
});

it('restores narrow saved dynamics nodes to the shared horizontal face without changing the document', () => {
  const graph = largeGraph(1);
  const node = graph.nodes[0];
  assert(node);
  node.appearance.dynamics = {
    nodeId: node.id,
    portId: 'audio',
    thresholdId: 'thresholdDb',
    inputLabel: 'Input',
    outputLabel: 'Output',
    reductionLabel: 'Reduction',
    unavailableLabel: 'Waiting',
  };
  node.size = { width: 228, height: 160 };
  expect(graphNodeDefaultWidth(node)).toBe(420);
  expect(reconcileAudioFlowNodes([], graph)[0]?.width).toBe(420);
  expect(node.size).toEqual({ width: 228, height: 160 });
  const resized = resizeAudioGraphNode(graph, node.id, { width: 300, height: 180 });
  expect(resized.nodes[0]?.size).toEqual({ width: 420, height: 180 });
  expect(resized.revision).toBe(graph.revision);
  const roomy = { width: 560, height: 200 };
  expect(normalizeAudioGraphNodeSize(node, roomy)).toBe(roomy);
});

it('uses instance display names in graph accessibility labels without changing source names', () => {
  const graph = {
    revision: 1,
    edges: [],
    nodes: [
      {
        id: 'video',
        appearance: { title: 'Firefox', category: 'Application', icon: 'APP' },
        displayName: 'Video',
        inputPorts: [],
        outputPorts: [],
        position: { x: 0, y: 0 },
      },
    ],
  };
  const named = reconcileAudioFlowNodes([], graph);
  expect(named[0]?.ariaLabel).toBe('Video');
  expect(named[0]?.data.graphNode.appearance.title).toBe('Firefox');
  const restored = reconcileAudioFlowNodes(named, {
    ...graph,
    nodes: graph.nodes.map((node) => ({ ...node, displayName: undefined })),
  });
  expect(restored[0]?.ariaLabel).toBe('Firefox');
});

it('names each cable by display name and channel, and relabels it when a node is renamed', () => {
  const graph: AudioGraphModel = {
    ...largeGraph(2),
    nodes: largeGraph(2).nodes.map((node, index) =>
      index === 0
        ? { ...node, outputPorts: [{ id: 'out', label: 'Output', channelIndex: 1 }] }
        : node,
    ),
  };
  const label = (value: {
    source: string;
    sourceChannel: number;
    target: string;
    targetChannel: number;
  }) => `${value.source} ${value.sourceChannel} > ${value.target} ${value.targetChannel}`;
  const edges = reconcileAudioFlowEdges([], graph, label);
  expect(edges[0]?.ariaLabel).toBe('0 2 > 1 1');
  expect(reconcileAudioFlowEdges(edges, graph, label)).toBe(edges);

  const renamed: AudioGraphModel = {
    ...graph,
    nodes: graph.nodes.map((node, index) => (index === 1 ? { ...node, displayName: 'Mix' } : node)),
  };
  const relabeled = reconcileAudioFlowEdges(edges, renamed, label);
  expect(relabeled[0]).not.toBe(edges[0]);
  expect(relabeled[0]?.ariaLabel).toBe('0 2 > Mix 1');
  expect(reconcileAudioFlowEdges([], graph)[0]?.ariaLabel).toBeUndefined();

  const [edge] = graph.edges;
  assert(edge);
  const dangling: AudioGraphModel = {
    ...graph,
    edges: [
      { ...edge, id: 'missing-port', sourcePortId: 'gone' },
      { ...edge, id: 'missing-node', targetNodeId: 'gone' },
    ],
  };
  // React Flow keeps its own fallback when an endpoint is not in the model.
  expect(reconcileAudioFlowEdges([], dangling, label).map((flow) => flow.ariaLabel)).toEqual([
    undefined,
    undefined,
  ]);
});

it('adds the port name only where a channel number repeats across buses', () => {
  const base = largeGraph(2);
  const [source, mixer] = base.nodes;
  assert(source && mixer);
  const graph: AudioGraphModel = {
    ...base,
    nodes: [
      source,
      {
        ...mixer,
        inputPorts: [
          { id: 'a::0', label: 'Mixer input 1 1', channelIndex: 0 },
          { id: 'b::0', label: 'Mixer input 2 1', channelIndex: 0 },
        ],
      },
    ],
    edges: [
      {
        id: 'to-a',
        sourceNodeId: 'node-0',
        sourcePortId: 'out',
        targetNodeId: 'node-1',
        targetPortId: 'a::0',
      },
      {
        id: 'to-b',
        sourceNodeId: 'node-0',
        sourcePortId: 'out',
        targetNodeId: 'node-1',
        targetPortId: 'b::0',
      },
    ],
  };
  const nodesById = new Map(graph.nodes.map((node) => [node.id, node]));
  expect(graph.edges.map((edge) => audioGraphEdgeEndpoints(nodesById, edge))).toEqual([
    { source: '0', sourceChannel: 1, target: '1', targetChannel: 1, targetPort: 'Mixer input 1 1' },
    { source: '0', sourceChannel: 1, target: '1', targetChannel: 1, targetPort: 'Mixer input 2 1' },
  ]);
});

it('numbers ports that repeat both a bus name and a channel so every name is unique', () => {
  expect(
    namedAudioPorts([
      { id: 'a', label: 'Main', channelIndex: 0 },
      { id: 'b', label: 'Main', channelIndex: 1 },
      { id: 'c', label: 'Main', channelIndex: 0 },
      { id: 'd', label: 'Sidechain', channelIndex: 0 },
      { id: 'e', label: '' },
      { id: 'f', label: '' },
    ]).map(({ name, channel }) => [name, channel]),
  ).toEqual([
    ['Main (1)', 1],
    ['Main', 2],
    ['Main (2)', 1],
    ['Sidechain', 1],
    ['', 5],
    ['', 6],
  ]);
});

it('keeps generated port ordinals unique and stable when ports reorder', () => {
  const ports = [
    { id: 'b', label: 'Main', channelIndex: 0 },
    { id: 'a', label: 'Main', channelIndex: 0 },
    { id: 'c', label: 'Main (2)', channelIndex: 0 },
  ];
  const names = (list: typeof ports) =>
    Object.fromEntries(namedAudioPorts(list).map(({ port, name }) => [port.id, name]));
  expect(names(ports)).toEqual({ a: 'Main (1)', b: 'Main (3)', c: 'Main (2)' });
  expect(names([...ports].reverse())).toEqual(names(ports));
});
