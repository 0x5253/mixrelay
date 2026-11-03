import type { Edge, Node } from '@xyflow/react';
import type { GraphEdge } from '../domain/graph/topology';
import { canvasLayers } from '../theme/tokens';

import {
  type AudioGraphModel,
  type AudioGraphNode,
  audioGraphNodeTitle,
  normalizeAudioGraphNodeSize,
} from './model';
import { type AudioGraphEdgeEndpoints, audioGraphEdgeEndpoints } from './portNames';

export interface AudioFlowNodeData extends Record<string, unknown> {
  graphNode: AudioGraphNode;
}

interface AudioFlowEdgeData extends Record<string, unknown> {
  graphEdge: GraphEdge;
}

export type AudioFlowNode = Node<AudioFlowNodeData, 'audio'>;
export type AudioFlowEdge = Edge<AudioFlowEdgeData>;

/** Open disclosures stay above selected nodes and below portalled menus. */
export function applyAudioNodeLayers(nodes: AudioFlowNode[], openedNodeId: string | null) {
  return nodes.map((node) => {
    const zIndex =
      node.id === openedNodeId
        ? canvasLayers.nodeOptionsOpen
        : node.selected
          ? canvasLayers.nodeSelected
          : canvasLayers.node;
    return node.zIndex === zIndex ? node : { ...node, zIndex };
  });
}

function createFlowNode(graphNode: AudioGraphNode, previous?: AudioFlowNode): AudioFlowNode {
  return {
    ...previous,
    data: { graphNode },
    deletable: true,
    ariaLabel: audioGraphNodeTitle(graphNode),
    domAttributes: { 'aria-description': graphNode.appearance.deviceAppActivity?.summary },
    id: graphNode.id,
    position: graphNode.position,
    width: normalizeAudioGraphNodeSize(graphNode, graphNode.size)?.width,
    // The card owns its collapsed size and temporary disclosure height. Let
    // React Flow measure that rendered height instead of fixing the outer box.
    height: undefined,
    type: 'audio',
    zIndex: canvasLayers.node,
  };
}

export type AudioFlowEdgeLabel = (endpoints: AudioGraphEdgeEndpoints) => string;

function createFlowEdge(
  graphEdge: GraphEdge,
  previous: AudioFlowEdge | undefined,
  ariaLabel: string | undefined,
): AudioFlowEdge {
  return {
    ...previous,
    ariaLabel,
    data: { graphEdge },
    id: graphEdge.id,
    reconnectable: true,
    source: graphEdge.sourceNodeId,
    sourceHandle: graphEdge.sourcePortId,
    target: graphEdge.targetNodeId,
    targetHandle: graphEdge.targetPortId,
    type: 'default',
    zIndex: canvasLayers.cable,
  };
}

export function reconcileAudioFlowNodes(
  current: AudioFlowNode[],
  model: AudioGraphModel,
): AudioFlowNode[] {
  const currentById = new Map(current.map((node) => [node.id, node]));
  const next = model.nodes.map((graphNode) => {
    const previous = currentById.get(graphNode.id);
    return previous?.data.graphNode === graphNode ? previous : createFlowNode(graphNode, previous);
  });
  return next.length === current.length && next.every((node, index) => node === current[index])
    ? current
    : next;
}

export function reconcileAudioFlowEdges(
  current: AudioFlowEdge[],
  model: AudioGraphModel,
  edgeLabel?: AudioFlowEdgeLabel,
): AudioFlowEdge[] {
  const currentById = new Map(current.map((edge) => [edge.id, edge]));
  const nodesById = new Map(edgeLabel ? model.nodes.map((node) => [node.id, node]) : []);
  const next = model.edges.map((graphEdge) => {
    const previous = currentById.get(graphEdge.id);
    const endpoints = edgeLabel ? audioGraphEdgeEndpoints(nodesById, graphEdge) : null;
    const ariaLabel = endpoints && edgeLabel ? edgeLabel(endpoints) : undefined;
    return previous?.data?.graphEdge === graphEdge && previous.ariaLabel === ariaLabel
      ? previous
      : createFlowEdge(graphEdge, previous, ariaLabel);
  });
  return next.length === current.length && next.every((edge, index) => edge === current[index])
    ? current
    : next;
}
