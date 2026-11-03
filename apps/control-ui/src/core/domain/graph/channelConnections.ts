import {
  type GraphConnection,
  type GraphConnectionRejection,
  type GraphEdge,
  type GraphTopology,
  graphConnectionId,
  graphConnectionRejection,
} from './topology';

/** Source channels of one node mapped onto a target node from an anchor pair. */
export interface ChannelConnectionRequest {
  sourceNodeId: string;
  /** Selected output ports; gaps between them are preserved on the target side. */
  sourcePortIds: readonly string[];
  /** The source port that lands on `targetAnchorPortId`. */
  sourceAnchorPortId: string;
  targetNodeId: string;
  targetAnchorPortId: string;
}

export type ChannelConnectionSkip = GraphConnectionRejection | 'out-of-range';

export interface ChannelConnectionPlan {
  /** Connections that will be created, in source port order. */
  accepted: GraphConnection[];
  /** Source ports that will not connect, with the reason. */
  skipped: { sourcePortId: string; targetPortId?: string; reason: ChannelConnectionSkip }[];
}

/** The single mapping used by pointer drags and the keyboard connection dialog: each
 * selected source port keeps its offset from the source anchor on the target's input
 * ports. Pairs whose target position does not exist are reported as out of range. */
export function mapChannelConnections(graph: GraphTopology, request: ChannelConnectionRequest) {
  const pairs: GraphConnection[] = [];
  const outOfRange: string[] = [];
  const invalid: string[] = [];
  const source = graph.nodes.find((node) => node.id === request.sourceNodeId);
  const target = graph.nodes.find((node) => node.id === request.targetNodeId);
  if (!source || !target) return { pairs, outOfRange, invalid: [...request.sourcePortIds] };
  const sourceAnchor = source.outputPorts.findIndex(
    (port) => port.id === request.sourceAnchorPortId,
  );
  const targetAnchor = target.inputPorts.findIndex(
    (port) => port.id === request.targetAnchorPortId,
  );
  const selected = new Set(request.sourcePortIds);
  for (const [index, port] of source.outputPorts.entries()) {
    if (!selected.has(port.id)) continue;
    const targetPort =
      sourceAnchor < 0 || targetAnchor < 0
        ? undefined
        : target.inputPorts[targetAnchor + index - sourceAnchor];
    if (targetPort)
      pairs.push({
        sourceNodeId: source.id,
        sourcePortId: port.id,
        targetNodeId: target.id,
        targetPortId: targetPort.id,
      });
    else outOfRange.push(port.id);
  }
  for (const sourcePortId of request.sourcePortIds)
    if (!source.outputPorts.some((port) => port.id === sourcePortId)) invalid.push(sourcePortId);
  return { pairs, outOfRange, invalid };
}

/** Maps the request and validates every pair against the graph, including pairs
 * accepted earlier in the same request, so duplicates are reported, not created twice. */
export function planChannelConnections(
  graph: GraphTopology,
  request: ChannelConnectionRequest,
): ChannelConnectionPlan {
  const { pairs, outOfRange, invalid } = mapChannelConnections(graph, request);
  const plan: ChannelConnectionPlan = {
    accepted: [],
    skipped: [
      ...outOfRange.map((sourcePortId) => ({ sourcePortId, reason: 'out-of-range' as const })),
      ...invalid.map((sourcePortId) => ({ sourcePortId, reason: 'invalid' as const })),
    ],
  };
  const edges: GraphEdge[] = [...graph.edges];
  const pending: GraphTopology = { ...graph, edges };
  for (const connection of pairs) {
    const reason = graphConnectionRejection(pending, connection);
    if (reason) {
      plan.skipped.push({
        sourcePortId: connection.sourcePortId,
        targetPortId: connection.targetPortId,
        reason,
      });
      continue;
    }
    plan.accepted.push(connection);
    edges.push({ id: graphConnectionId(connection), ...connection });
  }
  return plan;
}
