import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { RuntimeGraphEdge } from '../../core/domain/control/graph';
import type { GraphEdge } from '../../core/domain/graph/topology';

/** A missing processor descriptor changes visibility, not the authoritative routes.
 * Only explicitly edited cable groups are converted back to per-channel routes. */
export function serializeGraphEdges(
  model: AudioGraphModel,
  retainedNodes: ReadonlySet<string>,
  serializeEdited: (edge: GraphEdge) => RuntimeGraphEdge | null,
): RuntimeGraphEdge[] {
  const edited = new Map(model.edges.map((edge) => [edge.id, edge]));
  const preserved: RuntimeGraphEdge[] = [];
  for (const projection of model.edgeProjections ?? []) {
    if (
      !retainedNodes.has(projection.edge.source.nodeId) ||
      !retainedNodes.has(projection.edge.destination.nodeId)
    )
      continue;
    if (!projection.views.every((view) => sameCable(view, edited.get(view.id)))) continue;
    const available = retainKnownRoutes(model, projection.edge);
    if (available) preserved.push(available);
    for (const view of projection.views) edited.delete(view.id);
  }
  const reserved = new Set(preserved.map((edge) => edge.id));
  return [
    ...preserved,
    ...Array.from(edited.values()).flatMap((view) => {
      const edge = serializeEdited(view);
      if (!edge) return [];
      let id = edge.id;
      if (reserved.has(id)) id = `@route:${JSON.stringify([id])}`;
      while (reserved.has(id)) id = `#${id}`;
      reserved.add(id);
      return [id === edge.id ? edge : { ...edge, id }];
    }),
  ];
}

function sameCable(previous: GraphEdge, current: GraphEdge | undefined) {
  return (
    current !== undefined &&
    previous.sourceNodeId === current.sourceNodeId &&
    previous.sourcePortId === current.sourcePortId &&
    previous.targetNodeId === current.targetNodeId &&
    previous.targetPortId === current.targetPortId
  );
}

/** Virtual references and recorder inputs have authoritative geometry even when a provider is missing. */
function retainKnownRoutes(
  model: AudioGraphModel,
  edge: RuntimeGraphEdge,
): RuntimeGraphEdge | null {
  const source = knownChannels(model, edge.source, 'outputPorts');
  const destination = knownChannels(model, edge.destination, 'inputPorts');
  if (source?.size === 0 || destination?.size === 0) return null;
  if (edge.channelMapping.mode === 'auto') return edge;
  const routes = edge.channelMapping.routes.filter(
    (route) =>
      (source === undefined || source.has(route.sourceChannel)) &&
      (destination === undefined || destination.has(route.destinationChannel)),
  );
  if (!routes.length) return null;
  return routes.length === edge.channelMapping.routes.length
    ? edge
    : { ...edge, channelMapping: { mode: 'explicit', routes } };
}

function knownChannels(
  model: AudioGraphModel,
  endpoint: RuntimeGraphEdge['source'],
  direction: 'inputPorts' | 'outputPorts',
): Set<number> | undefined {
  const references = model.nodes.filter(
    (node) =>
      (node.runtime?.type === 'virtual_endpoint' &&
        node.runtime.binding?.nodeId === endpoint.nodeId) ||
      (node.runtime?.type === 'recorder' && node.id === endpoint.nodeId),
  );
  if (!references.length) return undefined;
  return new Set(
    references.flatMap((node) =>
      node[direction].flatMap((port) =>
        port.runtimePortId === endpoint.portId && port.channelIndex !== undefined
          ? [port.channelIndex]
          : [],
      ),
    ),
  );
}
