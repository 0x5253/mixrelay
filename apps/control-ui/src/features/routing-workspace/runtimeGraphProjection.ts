import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import type {
  RuntimeChannelRoute,
  RuntimeGraphDocument,
  RuntimeGraphEdge,
  RuntimeGraphNode,
} from '../../core/domain/control/graph';
import type { ExternalProcessorDescriptor } from '../../core/domain/control/processors';
import { directNumber, objectValue } from './domain/configValues';
import { serializeGraphEdges } from './graphEdgeProjection';
import { defaultPresentation, type GraphPresentation } from './graphPresentation';
import { CHANNEL_PORT_SEPARATOR, type RuntimeAudioNode, runtimeNodeId } from './runtimeAudioNode';
import { runtimeNodeToAudioNodes } from './runtimeNodeParsing';
import { TO_OS_PORT_ID } from './virtualEndpointNodes';

/** The canvas view of an engine document: one card per node side and one cable per channel. */
export function runtimeGraphToAudioGraph(
  document: RuntimeGraphDocument,
  presentation: GraphPresentation = defaultPresentation,
  processors: readonly ExternalProcessorDescriptor[] = [],
): AudioGraphModel {
  const reservedIds = new Set(document.nodes.map((node) => node.id));
  const nodes = document.nodes.flatMap((node, index) =>
    runtimeNodeToAudioNodes(node, index, presentation, processors, reservedIds).map((view) => ({
      ...view,
      runtime: { ...view.runtime, control: node.control },
    })),
  );
  const projected = document.edges.map((edge) => ({
    edge,
    views: runtimeEdgeToAudioEdges(edge, nodes),
  }));
  return {
    edges: projected.flatMap(({ views }) => views),
    edgeProjections: projected,
    nodes,
    revision: document.revision,
  };
}

/** The engine document of a canvas model; `source` supplies what the canvas does not author. */
export function audioGraphToRuntimeGraph(
  model: AudioGraphModel,
  source: RuntimeGraphDocument,
): RuntimeGraphDocument {
  const sourceNodes = new Map(source.nodes.map((node) => [node.id, node]));
  const includedNodes = new Map<string, RuntimeGraphNode>();
  for (const node of model.nodes) {
    const id = runtimeNodeId(node);
    const runtimeNode = audioNodeToRuntimeNode(node, model, sourceNodes.get(id));
    if (runtimeNode) includedNodes.set(id, runtimeNode);
  }
  return {
    edges: serializeGraphEdges(model, new Set(includedNodes.keys()), (edge) =>
      runtimeEdge(model, edge),
    ),
    nodes: [...includedNodes.values()],
    revision: model.revision,
    schemaVersion: 2,
  };
}

function audioNodeToRuntimeNode(
  node: AudioGraphNode,
  model: AudioGraphModel,
  sourceNode?: RuntimeGraphNode,
): RuntimeGraphNode | null {
  const id = runtimeNodeId(node);
  if (!node.runtime) return null;
  if (node.runtime.type !== 'virtual_endpoint') {
    return {
      ...(node.runtime.control ? { control: node.runtime.control } : {}),
      config: node.runtime.config,
      id,
      label: node.runtime.label === undefined ? node.appearance.title : node.runtime.label,
      type: node.runtime.type,
    };
  }
  const config = { ...(objectValue(node.runtime.config) ?? {}) };
  const endpointNodes = model.nodes.filter(
    (candidate) =>
      runtimeNodeId(candidate) === id && candidate.runtime?.type === 'virtual_endpoint',
  );
  // Both visible sides serialize into one endpoint. Its source owns the gain; do
  // not let the later sink card overwrite a live gain with its older config copy.
  const source = endpointNodes.find((candidate) => candidate.runtime?.binding?.side === 'source');
  const sourceGain = directNumber(source?.runtime?.config, 'gainDb');
  if (sourceGain !== null) config.gainDb = sourceGain;
  config.reference = {
    render: Boolean(source),
    capture: endpointNodes.some((candidate) => candidate.runtime?.binding?.side === 'sink'),
  };
  return {
    ...(node.runtime.control ? { control: node.runtime.control } : {}),
    config,
    id,
    label: sourceNode ? sourceNode.label : node.appearance.title,
    type: node.runtime.type,
  };
}

function runtimeEdge(
  model: AudioGraphModel,
  edge: AudioGraphModel['edges'][number],
): RuntimeGraphEdge | null {
  const source = channelBinding(model, edge.sourceNodeId, edge.sourcePortId, 'outputPorts');
  const destination = channelBinding(model, edge.targetNodeId, edge.targetPortId, 'inputPorts');
  if (!source || !destination) return null;
  return {
    channelMapping: {
      mode: 'explicit',
      routes: [
        {
          destinationChannel: destination.channelIndex,
          sourceChannel: source.channelIndex,
        },
      ],
    },
    destination: {
      nodeId: destination.nodeId,
      portId: destination.runtimePortId,
    },
    id: edge.id,
    source: { nodeId: source.nodeId, portId: source.runtimePortId },
  };
}

function runtimeEdgeToAudioEdges(
  edge: RuntimeGraphEdge,
  nodes: readonly RuntimeAudioNode[],
): AudioGraphModel['edges'] {
  const sourceNodeId = graphNodeId(nodes, edge.source.nodeId, edge.source.portId);
  const targetNodeId = graphNodeId(nodes, edge.destination.nodeId, edge.destination.portId);
  const routes = runtimeRoutes(edge, nodes, sourceNodeId, targetNodeId);
  return routes.flatMap((route, routeIndex) => {
    const sourcePortId = channelPortFor(
      nodes,
      sourceNodeId,
      edge.source.portId,
      route.sourceChannel,
      'outputPorts',
    );
    const targetPortId = channelPortFor(
      nodes,
      targetNodeId,
      edge.destination.portId,
      route.destinationChannel,
      'inputPorts',
    );
    if (!sourcePortId || !targetPortId) return [];
    return [
      {
        id: `${edge.id}${CHANNEL_PORT_SEPARATOR}${routeIndex}`,
        sourceNodeId,
        sourcePortId,
        targetNodeId,
        targetPortId,
      },
    ];
  });
}

function runtimeRoutes(
  edge: RuntimeGraphEdge,
  nodes: readonly RuntimeAudioNode[],
  sourceNodeId: string,
  targetNodeId: string,
): RuntimeChannelRoute[] {
  if (edge.channelMapping.mode === 'explicit') return edge.channelMapping.routes;
  const sourceChannels = portChannelCount(nodes, sourceNodeId, edge.source.portId, 'outputPorts');
  const targetChannels = portChannelCount(
    nodes,
    targetNodeId,
    edge.destination.portId,
    'inputPorts',
  );
  return Array.from({ length: Math.min(sourceChannels, targetChannels) }, (_, channel) => ({
    destinationChannel: channel,
    sourceChannel: channel,
  }));
}

function channelBinding(
  model: AudioGraphModel,
  nodeId: string,
  visualPortId: string,
  direction: 'inputPorts' | 'outputPorts',
): { nodeId: string; channelIndex: number; runtimePortId: string } | null {
  const node = model.nodes.find((candidate) => candidate.id === nodeId);
  const port = node?.[direction].find((candidate) => candidate.id === visualPortId);
  if (!node || !port?.runtimePortId || port.channelIndex === undefined) return null;
  return {
    nodeId: runtimeNodeId(node),
    channelIndex: port.channelIndex,
    runtimePortId: port.runtimePortId,
  };
}

function channelPortFor(
  nodes: readonly RuntimeAudioNode[],
  nodeId: string,
  runtimePortId: string,
  channelIndex: number,
  direction: 'inputPorts' | 'outputPorts',
): string | null {
  const port = nodes
    .find((node) => node.id === nodeId)
    ?.[direction].find(
      (candidate) =>
        candidate.runtimePortId === runtimePortId && candidate.channelIndex === channelIndex,
    );
  return port?.id ?? null;
}

function portChannelCount(
  nodes: readonly RuntimeAudioNode[],
  nodeId: string,
  runtimePortId: string,
  direction: 'inputPorts' | 'outputPorts',
): number {
  return (
    nodes
      .find((node) => node.id === nodeId)
      ?.[direction].filter((port) => port.runtimePortId === runtimePortId).length ?? 0
  );
}

function graphNodeId(nodes: readonly AudioGraphNode[], nodeId: string, portId: string): string {
  const side = portId === TO_OS_PORT_ID ? 'sink' : 'source';
  return (
    nodes.find(
      (node) => node.runtime?.binding?.nodeId === nodeId && node.runtime.binding.side === side,
    )?.id ?? nodeId
  );
}

export const routingLayoutProjection = {
  nodeIds: (document: RuntimeGraphDocument): ReadonlySet<string> =>
    new Set(runtimeGraphToAudioGraph(document).nodes.map((node) => node.id)),
};
