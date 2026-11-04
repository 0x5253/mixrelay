import type { Connection, Edge, ReactFlowProps } from '@xyflow/react';
import { type MouseEvent as ReactMouseEvent, useCallback } from 'react';
import { planChannelConnections } from '../domain/graph/channelConnections';
import {
  canConnectGraph,
  connectGraphBatch,
  disconnectGraph,
  reconnectGraph,
  removeGraphItems,
} from '../domain/graph/topology';
import type { AudioGraphChangeKind } from './AudioGraphCanvas';
import { dragConnectionRequest } from './connectionPlanning';
import type { AudioGraphModel } from './model';
import type { SelectedAudioPort } from './portSelection';
import { type AudioFlowEdge, type AudioFlowNode, toGraphConnection } from './reactFlowAdapter';

export type CommitGraph = (nextModel: AudioGraphModel, kind: AudioGraphChangeKind) => void;
type ReconnectEnd = NonNullable<ReactFlowProps<AudioFlowNode, AudioFlowEdge>['onReconnectEnd']>;

/** Turns React Flow drags into channel connections, including multi-channel batches. */
export function useConnectionHandlers(
  model: AudioGraphModel,
  commit: CommitGraph,
  selectedPortsRef: { current: SelectedAudioPort[] },
  clearSelectedPorts: () => void,
) {
  const isValidConnection = useCallback(
    (connection: Connection | Edge) =>
      canConnectGraph(
        model,
        toGraphConnection(connection),
        'id' in connection ? connection.id : undefined,
      ),
    [model],
  );
  const onConnect = useCallback(
    (connection: Connection) => {
      const { accepted: connections } = planChannelConnections(
        model,
        dragConnectionRequest(model, toGraphConnection(connection), selectedPortsRef.current),
      );
      const nextModel = connectGraphBatch(model, connections);
      commit(nextModel, 'runtime');
      if (nextModel !== model && connections.length > 1) clearSelectedPorts();
    },
    [clearSelectedPorts, commit, model, selectedPortsRef],
  );
  const onReconnect = useCallback(
    (edge: AudioFlowEdge, connection: Connection) =>
      commit(reconnectGraph(model, edge.id, toGraphConnection(connection)), 'runtime'),
    [commit, model],
  );
  const onReconnectEnd = useCallback<ReconnectEnd>(
    (_, edge, _handleType, connectionState) => {
      if (!connectionState.isValid) commit(disconnectGraph(model, [edge.id]), 'runtime');
    },
    [commit, model],
  );
  return { isValidConnection, onConnect, onReconnect, onReconnectEnd };
}

/** Removes the deleted selection, or a cable on double-click, as one runtime edit. */
export function useDeletionHandlers(model: AudioGraphModel, commit: CommitGraph) {
  const onDelete = useCallback(
    ({ nodes, edges }: { nodes: AudioFlowNode[]; edges: AudioFlowEdge[] }) =>
      commit(
        removeGraphItems(
          model,
          nodes.map((node) => node.id),
          edges.map((edge) => edge.id),
        ),
        'runtime',
      ),
    [commit, model],
  );
  const onEdgeDoubleClick = useCallback(
    (_: ReactMouseEvent, edge: AudioFlowEdge) =>
      commit(disconnectGraph(model, [edge.id]), 'runtime'),
    [commit, model],
  );
  return { onDelete, onEdgeDoubleClick };
}
