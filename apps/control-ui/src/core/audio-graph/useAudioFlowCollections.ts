import { type NodeChange, useEdgesState, useNodesState } from '@xyflow/react';
import { useCallback, useEffect, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useStructuralShare } from '../ui/useStructuralShare';
import type { AudioGraphModel } from './model';
import { applyNodeLayoutChanges } from './nodeChanges';
import type { AudioGraphEdgeEndpoints } from './portNames';
import {
  type AudioFlowEdge,
  type AudioFlowEdgeLabel,
  type AudioFlowNode,
  applyAudioNodeLayers,
  pinMeasuredWidthOnly,
  reconcileAudioFlowEdges,
  reconcileAudioFlowNodes,
  selectFlowEdges,
} from './reactFlowAdapter';
import type { CommitGraph } from './useConnectionHandlers';

function useEdgeLabel(): AudioFlowEdgeLabel {
  const { t } = useTranslation();
  return useCallback(
    ({ source, sourcePort, target, targetPort, ...channels }: AudioGraphEdgeEndpoints) =>
      t('routing.a11y.edgeLabel', {
        ...channels,
        // Bus names disambiguate repeated channel numbers, matching the port's own name.
        source: sourcePort ? `${source} · ${sourcePort}` : source,
        target: targetPort ? `${target} · ${targetPort}` : target,
      }),
    [t],
  );
}

/** React Flow's node and edge state, reconciled from the model and committing layout. */
export function useAudioFlowCollections(
  presented: AudioGraphModel,
  commit: CommitGraph,
  disclosureHeights: Map<string, number>,
  openedNodeId: string | null,
) {
  // Presenters rebuild node objects on every status poll; equal nodes keep their identity
  // so memoized node views and React Flow skip them.
  const model = useStructuralShare(presented);
  const edgeLabel = useEdgeLabel();
  const initialNodes = useRef<AudioFlowNode[] | null>(null);
  const initialEdges = useRef<AudioFlowEdge[] | null>(null);
  initialNodes.current ??= reconcileAudioFlowNodes([], model);
  initialEdges.current ??= reconcileAudioFlowEdges([], model, edgeLabel);
  const [nodes, setNodes, applyNodeChanges] = useNodesState<AudioFlowNode>(initialNodes.current);
  const [edges, setEdges, applyEdgeChanges] = useEdgesState<AudioFlowEdge>(initialEdges.current);

  useEffect(
    () => setNodes((current) => reconcileAudioFlowNodes(current, model)),
    [model, setNodes],
  );
  useEffect(
    () => setEdges((current) => reconcileAudioFlowEdges(current, model, edgeLabel)),
    [edgeLabel, model, setEdges],
  );
  const onNodesChange = useCallback(
    (changes: NodeChange<AudioFlowNode>[]) => {
      applyNodeChanges(pinMeasuredWidthOnly(changes));
      commit(applyNodeLayoutChanges(model, changes, disclosureHeights), 'layout');
    },
    [applyNodeChanges, commit, disclosureHeights, model],
  );
  const layeredNodes = useMemo(
    () => applyAudioNodeLayers(nodes, openedNodeId),
    [nodes, openedNodeId],
  );
  const selectEdges = useCallback(
    (edgeIds: readonly string[]) => {
      if (edgeIds.length === 0) return;
      const selectedIds = new Set(edgeIds);
      requestAnimationFrame(() => setEdges((current) => selectFlowEdges(current, selectedIds)));
    },
    [setEdges],
  );
  return {
    applyEdgeChanges,
    onNodesChange,
    edges,
    nodes: layeredNodes,
    selectEdges,
    setEdges,
    setNodes,
  };
}
