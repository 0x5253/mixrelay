import {
  type ChannelConnectionRequest,
  mapChannelConnections,
} from '../domain/graph/channelConnections';
import type { GraphConnection } from '../domain/graph/topology';
import type { AudioGraphModel, AudioGraphPort } from './model';
import type { SelectedAudioPort } from './portSelection';

/** The node's output ports that are selected, in port order. */
export function selectedOutputPorts(
  model: AudioGraphModel,
  selectedPorts: readonly SelectedAudioPort[],
  nodeId: string,
): AudioGraphPort[] {
  const node = model.nodes.find((candidate) => candidate.id === nodeId);
  if (!node) return [];
  const selectedIds = new Set(
    selectedPorts
      .filter((port) => port.nodeId === nodeId && port.side === 'output')
      .map((port) => port.portId),
  );
  return node.outputPorts.filter((port) => selectedIds.has(port.id));
}

/** A drag from `connection`'s source port carries the other selected outputs of that node
 * when the grabbed port is part of the selection; otherwise only the grabbed port. */
export function dragConnectionRequest(
  model: AudioGraphModel,
  connection: GraphConnection,
  selectedPorts: readonly SelectedAudioPort[],
): ChannelConnectionRequest {
  const selected = selectedOutputPorts(model, selectedPorts, connection.sourceNodeId);
  const batch = selected.length > 1 && selected.some((port) => port.id === connection.sourcePortId);
  return {
    sourceNodeId: connection.sourceNodeId,
    sourcePortIds: batch ? selected.map((port) => port.id) : [connection.sourcePortId],
    sourceAnchorPortId: connection.sourcePortId,
    targetNodeId: connection.targetNodeId,
    targetAnchorPortId: connection.targetPortId,
  };
}

/** The cables a drag would create. A target without the hovered port still previews the
 * grabbed cable alone; committing validates through planChannelConnections and creates nothing. */
export function selectedConnectionBatch(
  model: AudioGraphModel,
  connection: GraphConnection,
  selectedPorts: readonly SelectedAudioPort[],
): GraphConnection[] {
  const { pairs } = mapChannelConnections(
    model,
    dragConnectionRequest(model, connection, selectedPorts),
  );
  return pairs.length ? pairs : [connection];
}

export interface ConnectionPreviewPair {
  sourcePort: Pick<AudioGraphPort, 'id'>;
  targetPort?: Pick<AudioGraphPort, 'id'>;
}

/** The cables to draw while a drag is in progress: every carried source port, paired with
 * its target port once the pointer is over a target. */
export function connectionPreviewPairs(
  model: AudioGraphModel,
  selectedPorts: readonly SelectedAudioPort[],
  sourceNodeId: string,
  sourceAnchorId: string,
  targetNodeId?: string,
  targetAnchorId?: string,
): ConnectionPreviewPair[] {
  const selectedSources = selectedOutputPorts(model, selectedPorts, sourceNodeId);
  const activeSources =
    selectedSources.length > 1 && selectedSources.some((port) => port.id === sourceAnchorId)
      ? selectedSources
      : [{ id: sourceAnchorId }];
  if (!targetNodeId || !targetAnchorId) {
    return activeSources.map((sourcePort) => ({ sourcePort }));
  }
  return selectedConnectionBatch(
    model,
    {
      sourceNodeId,
      sourcePortId: sourceAnchorId,
      targetNodeId,
      targetPortId: targetAnchorId,
    },
    selectedPorts,
  ).map((connection) => ({
    sourcePort: { id: connection.sourcePortId },
    targetPort: { id: connection.targetPortId },
  }));
}
