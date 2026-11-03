import { expect, it } from 'vitest';

import {
  addGraphNode,
  canConnectGraph,
  connectGraph,
  connectGraphBatch,
  disconnectGraph,
  type GraphConnection,
  type GraphPort,
  type GraphTopology,
  reconnectGraph,
  removeGraphItems,
} from './topology';

interface TestNode {
  id: string;
  inputPorts: GraphPort[];
  outputPorts: GraphPort[];
}
type TestGraph = GraphTopology<TestNode>;

function node(id: string, inputPorts: string[], outputPorts: string[]): TestNode {
  return {
    id,
    inputPorts: inputPorts.map((portId) => ({ id: portId })),
    outputPorts: outputPorts.map((portId) => ({ id: portId })),
  };
}

function connection(
  sourceNodeId: string,
  targetNodeId: string,
  sourcePortId = 'out',
  targetPortId = 'in',
): GraphConnection {
  return { sourceNodeId, sourcePortId, targetNodeId, targetPortId };
}

function emptyModel(): TestGraph {
  return {
    revision: 4,
    nodes: [node('a', [], ['out']), node('b', ['in'], ['out']), node('c', ['in'], [])],
    edges: [],
  };
}

it('connects only valid output-to-input routes', () => {
  const model = emptyModel();
  const valid = connection('a', 'b');
  expect(canConnectGraph(model, valid)).toBe(true);
  expect(canConnectGraph(model, connection('a', 'a'))).toBe(false);
  expect(canConnectGraph(model, { ...valid, sourcePortId: '' })).toBe(false);
  expect(canConnectGraph(model, { ...valid, targetPortId: '' })).toBe(false);
  expect(canConnectGraph(model, connection('missing', 'b'))).toBe(false);
  expect(canConnectGraph(model, connection('a', 'missing'))).toBe(false);
  expect(canConnectGraph(model, connection('a', 'b', 'missing'))).toBe(false);
  expect(canConnectGraph(model, connection('a', 'b', 'out', 'missing'))).toBe(false);

  const connected = connectGraph(model, valid);
  expect(connected).not.toBe(model);
  expect(connected.revision).toBe(5);
  expect(connected.edges).toEqual([
    {
      id: 'edge:a:out:b:in',
      ...valid,
    },
  ]);
  expect(connectGraph(connected, valid)).toBe(connected);
  expect(canConnectGraph(connected, connection('b', 'b'))).toBe(false);
  expect(canConnectGraph(connected, connection('b', 'b', 'out', 'in'))).toBe(false);
});

it('allows input fan-in while rejecting duplicate edges and processing cycles', () => {
  const aToB = connectGraph(emptyModel(), connection('a', 'b'));
  expect(canConnectGraph(aToB, connection('a', 'b'))).toBe(false);
  expect(canConnectGraph(aToB, connection('b', 'b'))).toBe(false);

  const bToC = connectGraph(aToB, connection('b', 'c'));
  const withCyclePort: TestGraph = {
    ...bToC,
    nodes: bToC.nodes.map((item) =>
      item.id === 'a'
        ? { ...item, inputPorts: [{ id: 'in' }] }
        : item.id === 'c'
          ? { ...item, outputPorts: [{ id: 'out' }] }
          : item,
    ),
  };
  expect(canConnectGraph(withCyclePort, connection('c', 'a'))).toBe(false);

  const occupiedTarget = {
    ...emptyModel(),
    nodes: [...emptyModel().nodes, node('d', [], ['out'])],
    edges: [
      {
        id: 'existing',
        ...connection('a', 'b'),
      },
    ],
  };
  expect(canConnectGraph(occupiedTarget, connection('d', 'b'))).toBe(true);
  const fanIn = connectGraph(occupiedTarget, connection('d', 'b'));
  expect(fanIn.edges).toHaveLength(2);
  expect(fanIn.edges[0]).toEqual(occupiedTarget.edges[0]);
  expect(fanIn.edges[1]).toMatchObject(connection('d', 'b'));

  const convergingModel: TestGraph = {
    revision: 1,
    nodes: [
      node('source', [], ['out']),
      node('branch', ['in'], ['out']),
      node('left', ['in'], ['out']),
      node('right', ['in'], ['out']),
      node('join', ['left', 'right'], []),
    ],
    edges: [
      { id: 'branch-left', ...connection('branch', 'left') },
      { id: 'branch-right', ...connection('branch', 'right') },
      { id: 'left-join', ...connection('left', 'join', 'out', 'left') },
      { id: 'right-join', ...connection('right', 'join', 'out', 'right') },
    ],
  };
  expect(canConnectGraph(convergingModel, connection('source', 'branch'))).toBe(true);
});

it('treats host source and sink ports as graph boundaries instead of a processing cycle', () => {
  const endpoint = node('virtual-io', ['to_os'], ['from_os']);
  endpoint.inputPorts[0] = { boundary: 'sink', id: 'to_os' };
  endpoint.outputPorts[0] = { boundary: 'source', id: 'from_os' };
  const model: TestGraph = { revision: 1, nodes: [endpoint], edges: [] };

  expect(canConnectGraph(model, connection('virtual-io', 'virtual-io', 'from_os', 'to_os'))).toBe(
    true,
  );
  expect(
    connectGraph(model, connection('virtual-io', 'virtual-io', 'from_os', 'to_os')).edges,
  ).toHaveLength(1);

  const withProcessors: TestGraph = {
    revision: 1,
    nodes: [endpoint, node('left', ['in'], ['out']), node('right', ['in'], ['out'])],
    edges: [
      {
        id: 'host-boundary',
        ...connection('virtual-io', 'virtual-io', 'from_os', 'to_os'),
      },
    ],
  };
  expect(canConnectGraph(withProcessors, connection('left', 'right'))).toBe(true);
});

it('reconnects atomically while preserving the edge identity', () => {
  const connected = connectGraph(emptyModel(), connection('a', 'b'));
  const edgeId = connected.edges[0]?.id ?? '';
  expect(reconnectGraph(connected, 'missing', connection('b', 'c'))).toBe(connected);
  expect(reconnectGraph(connected, edgeId, connection('b', 'b'))).toBe(connected);

  const reconnected = reconnectGraph(connected, edgeId, connection('b', 'c'));
  expect(reconnected.revision).toBe(6);
  expect(reconnected.edges).toEqual([{ id: edgeId, ...connection('b', 'c') }]);

  const twoEdges = connectGraph(connected, connection('b', 'c'));
  const sameEndpoint = reconnectGraph(twoEdges, edgeId, connection('a', 'b'));
  expect(sameEndpoint.edges).toHaveLength(2);
  expect(sameEndpoint.edges[1]).toBe(twoEdges.edges[1]);
});

it('disconnects edge batches in one revision', () => {
  const connected = connectGraph(emptyModel(), connection('a', 'b'));
  expect(disconnectGraph(connected, ['missing'])).toBe(connected);
  const disconnected = disconnectGraph(connected, [connected.edges[0]?.id ?? '']);
  expect(disconnected.edges).toEqual([]);
  expect(disconnected.revision).toBe(6);
});

it('adds only valid unique nodes and removes nodes with incident edges atomically', () => {
  const model = emptyModel();
  const valid = node('d', ['left', 'right'], ['out']);
  expect(addGraphNode(model, valid).nodes.at(-1)).toBe(valid);
  expect(addGraphNode(model, node('a', [], []))).toBe(model);
  expect(addGraphNode(model, node('', [], []))).toBe(model);
  expect(addGraphNode(model, node('d', ['', 'right'], []))).toBe(model);
  expect(addGraphNode(model, node('d', ['same', 'same'], []))).toBe(model);
  expect(addGraphNode(model, node('d', [], ['']))).toBe(model);
  expect(addGraphNode(model, node('d', [], ['same', 'same']))).toBe(model);

  const connected = connectGraph(model, connection('a', 'b'));
  expect(removeGraphItems(connected, [])).toBe(connected);
  expect(removeGraphItems(connected, ['missing'], ['missing'])).toBe(connected);
  const withoutNode = removeGraphItems(connected, ['b']);
  expect(withoutNode.nodes.some((item) => item.id === 'b')).toBe(false);
  expect(withoutNode.edges).toEqual([]);
  expect(withoutNode.revision).toBe(6);

  const withoutEdge = removeGraphItems(connected, [], [connected.edges[0]?.id ?? '']);
  expect(withoutEdge.nodes).toBe(connected.nodes);
  expect(withoutEdge.edges).toEqual([]);
});

it('encodes endpoint components into deterministic edge IDs', () => {
  const model: TestGraph = {
    revision: 0,
    nodes: [node('source / one', [], ['out 1']), node('target', ['in/1'], [])],
    edges: [],
  };
  expect(
    connectGraph(model, connection('source / one', 'target', 'out 1', 'in/1')).edges[0]?.id,
  ).toBe('edge:source%20%2F%20one:out%201:target:in%2F1');
});

it('keeps the same graph when a channel batch contains no connections', () => {
  const model = emptyModel();
  expect(connectGraphBatch(model, [])).toBe(model);
  expect(model.revision).toBe(4);
});
