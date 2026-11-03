/** A port of a graph node. Boundary ports are where host audio enters or leaves the graph;
 * links between them do not order processing, so they cannot form a cycle. */
export interface GraphPort {
  boundary?: 'sink' | 'source';
  id: string;
}

export interface GraphNode {
  id: string;
  inputPorts: readonly GraphPort[];
  outputPorts: readonly GraphPort[];
}

export interface GraphConnection {
  sourceNodeId: string;
  sourcePortId: string;
  targetNodeId: string;
  targetPortId: string;
}

export interface GraphEdge extends GraphConnection {
  id: string;
}

/** Audio topology. The revision advances on every topology change and on nothing else. */
export interface GraphTopology<Node extends GraphNode = GraphNode> {
  edges: readonly GraphEdge[];
  nodes: readonly Node[];
  revision: number;
}

/** Why a connection cannot be made, or undefined when it can. `invalid` covers missing
 * nodes or ports and a processing self-connection; boundary ports may self-connect. */
export type GraphConnectionRejection = 'invalid' | 'existing' | 'cycle';

function hasValidPorts(ports: readonly GraphPort[]): boolean {
  const ids = ports.map((port) => port.id);
  return ids.every((id) => id.length > 0) && new Set(ids).size === ids.length;
}

function isValidNode(node: GraphNode): boolean {
  return node.id.length > 0 && hasValidPorts(node.inputPorts) && hasValidPorts(node.outputPorts);
}

function hasPort(ports: readonly GraphPort[], portId: string): boolean {
  return ports.some((port) => port.id === portId);
}

function portBoundary(
  graph: GraphTopology,
  nodeId: string,
  portId: string,
  direction: 'inputPorts' | 'outputPorts',
) {
  return graph.nodes
    .find((node) => node.id === nodeId)
    ?.[direction].find((port) => port.id === portId)?.boundary;
}

function ordersProcessing(graph: GraphTopology, connection: GraphConnection): boolean {
  return (
    portBoundary(graph, connection.sourceNodeId, connection.sourcePortId, 'outputPorts') !==
      'source' &&
    portBoundary(graph, connection.targetNodeId, connection.targetPortId, 'inputPorts') !== 'sink'
  );
}

function createsCycle(
  graph: GraphTopology,
  connection: GraphConnection,
  ignoredEdgeId?: string,
): boolean {
  const destinations = new Map<string, string[]>();
  for (const edge of graph.edges) {
    if (edge.id === ignoredEdgeId) continue;
    if (!ordersProcessing(graph, edge)) continue;
    const targets = destinations.get(edge.sourceNodeId) ?? [];
    targets.push(edge.targetNodeId);
    destinations.set(edge.sourceNodeId, targets);
  }
  const pending = [connection.targetNodeId];
  const visited = new Set<string>();
  for (let nodeId = pending.pop(); nodeId !== undefined; nodeId = pending.pop()) {
    if (visited.has(nodeId)) continue;
    if (nodeId === connection.sourceNodeId) return true;
    visited.add(nodeId);
    pending.push(...(destinations.get(nodeId) ?? []));
  }
  return false;
}

export function graphConnectionRejection(
  graph: GraphTopology,
  connection: GraphConnection,
  ignoredEdgeId?: string,
): GraphConnectionRejection | undefined {
  if (
    (connection.sourceNodeId === connection.targetNodeId && ordersProcessing(graph, connection)) ||
    connection.sourcePortId.length === 0 ||
    connection.targetPortId.length === 0
  ) {
    return 'invalid';
  }
  const source = graph.nodes.find((node) => node.id === connection.sourceNodeId);
  const target = graph.nodes.find((node) => node.id === connection.targetNodeId);
  if (
    !source ||
    !target ||
    !hasPort(source.outputPorts, connection.sourcePortId) ||
    !hasPort(target.inputPorts, connection.targetPortId)
  ) {
    return 'invalid';
  }
  const conflicts = graph.edges.some(
    (edge) =>
      edge.id !== ignoredEdgeId &&
      edge.sourceNodeId === connection.sourceNodeId &&
      edge.sourcePortId === connection.sourcePortId &&
      edge.targetNodeId === connection.targetNodeId &&
      edge.targetPortId === connection.targetPortId,
  );
  if (conflicts) return 'existing';
  if (ordersProcessing(graph, connection) && createsCycle(graph, connection, ignoredEdgeId))
    return 'cycle';
  return undefined;
}

export function canConnectGraph(
  graph: GraphTopology,
  connection: GraphConnection,
  ignoredEdgeId?: string,
): boolean {
  return graphConnectionRejection(graph, connection, ignoredEdgeId) === undefined;
}

export function graphConnectionId(connection: GraphConnection): string {
  return [
    'edge',
    connection.sourceNodeId,
    connection.sourcePortId,
    connection.targetNodeId,
    connection.targetPortId,
  ]
    .map(encodeURIComponent)
    .join(':');
}

export function connectGraph<Graph extends GraphTopology>(
  graph: Graph,
  connection: GraphConnection,
): Graph {
  return connectGraphBatch(graph, [connection]);
}

/** Each connection is validated against the graph including the ones added before it. */
export function connectGraphBatch<Graph extends GraphTopology>(
  graph: Graph,
  connections: readonly GraphConnection[],
): Graph {
  if (connections.length === 0) return graph;
  const edges = [...graph.edges];
  const pending = { ...graph, edges };
  for (const connection of connections) {
    if (!canConnectGraph(pending, connection)) continue;
    edges.push({ id: graphConnectionId(connection), ...connection });
  }
  if (edges.length === graph.edges.length) return graph;
  return { ...graph, edges, revision: graph.revision + 1 };
}

export function addGraphNode<Graph extends GraphTopology>(
  graph: Graph,
  node: Graph['nodes'][number],
): Graph {
  if (!isValidNode(node) || graph.nodes.some((existing) => existing.id === node.id)) return graph;
  return { ...graph, nodes: [...graph.nodes, node], revision: graph.revision + 1 };
}

/** Replaces one edge's endpoints and keeps its identity, or keeps the graph unchanged. */
export function reconnectGraph<Graph extends GraphTopology>(
  graph: Graph,
  edgeId: string,
  connection: GraphConnection,
): Graph {
  const edgeIndex = graph.edges.findIndex((edge) => edge.id === edgeId);
  if (edgeIndex < 0 || !canConnectGraph(graph, connection, edgeId)) return graph;
  return {
    ...graph,
    edges: graph.edges.map((edge, index) =>
      index === edgeIndex ? { id: edgeId, ...connection } : edge,
    ),
    revision: graph.revision + 1,
  };
}

export function disconnectGraph<Graph extends GraphTopology>(
  graph: Graph,
  edgeIds: readonly string[],
): Graph {
  const removedIds = new Set(edgeIds);
  const edges = graph.edges.filter((edge) => !removedIds.has(edge.id));
  return edges.length === graph.edges.length
    ? graph
    : { ...graph, edges, revision: graph.revision + 1 };
}

/** Removes nodes with every incident edge, plus the listed edges, in one revision. */
export function removeGraphItems<Graph extends GraphTopology>(
  graph: Graph,
  nodeIds: readonly string[],
  edgeIds: readonly string[] = [],
): Graph {
  const removedNodeIds = new Set(nodeIds);
  const removedEdgeIds = new Set(edgeIds);
  const nodes =
    removedNodeIds.size === 0
      ? graph.nodes
      : graph.nodes.filter((node) => !removedNodeIds.has(node.id));
  const edges =
    removedNodeIds.size === 0 && removedEdgeIds.size === 0
      ? graph.edges
      : graph.edges.filter(
          (edge) =>
            !removedEdgeIds.has(edge.id) &&
            !removedNodeIds.has(edge.sourceNodeId) &&
            !removedNodeIds.has(edge.targetNodeId),
        );
  return nodes.length === graph.nodes.length && edges.length === graph.edges.length
    ? graph
    : { ...graph, edges, nodes, revision: graph.revision + 1 };
}
