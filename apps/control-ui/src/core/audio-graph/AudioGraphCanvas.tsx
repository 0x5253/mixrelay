import '@xyflow/react/dist/base.css';

import { useCallback, useMemo, useRef, useState } from 'react';
import type { ProcessorEditorStatus } from '../domain/control/processors';
import { connectGraphBatch } from '../domain/graph/topology';
import { useMidiMappingActive } from '../midi/MidiContext';
import type { AudioGraphCanvasProps } from './audioGraphCanvasTypes';
import { ChannelConnectionDialog } from './ChannelConnectionDialog';
import { GraphCanvasBackground } from './GraphCanvasBackground';
import { GraphContextSurface } from './GraphContextSurface';
import { fitViewOptions, GraphFlowSurface } from './GraphFlowSurface';
import { AudioGraphProviders } from './graphCanvasContexts';
import type { useChannelConnectionRequests } from './graphKeyboard';
import type { AudioGraphModel } from './model';
import { nodeProcessorEditors } from './nodeViewModel';
import { PortSelectionFeedback } from './PortSelectionFeedback';
import { useAudioFlowCollections } from './useAudioFlowCollections';
import { useAudioPortSelection } from './useAudioPortSelection';
import { useCanvasViewport } from './useCanvasViewport';
import {
  type CommitGraph,
  useConnectionHandlers,
  useDeletionHandlers,
} from './useConnectionHandlers';
import { useGraphContextMenu } from './useGraphContextMenu';
import { useNodeInsertion } from './useNodeInsertion';
import { type NodeMenu, useNodeMenu } from './useNodeMenu';
import { useNodeRequests } from './useNodeRequests';

const emptyProcessorEditorStatuses = new Map<string, ProcessorEditorStatus>();

function useCanvasViewActions(
  onAddNode: AudioGraphCanvasProps['onAddNode'],
  nodeMenu: NodeMenu,
  clearPortSelection: () => void,
  onReveal: AudioGraphCanvasProps['onViewportChange'],
) {
  const placeNodes = useNodeInsertion(nodeMenu, onReveal);
  const addNode = (itemId: string) => onAddNode?.(itemId, nodeMenu.positionRef.current, placeNodes);
  const onPaneClick = useCallback(() => {
    nodeMenu.close();
    clearPortSelection();
  }, [clearPortSelection, nodeMenu.close]);
  return { addNode, onPaneClick };
}

/** Graph edits pause while MIDI mapping owns the surface; unchanged models are dropped. */
function useGraphCommit(
  model: AudioGraphModel,
  onModelChange: AudioGraphCanvasProps['onModelChange'],
): CommitGraph {
  const mapping = useMidiMappingActive();
  return useCallback(
    (nextModel, kind) => {
      if (!mapping && nextModel !== model) onModelChange(nextModel, kind);
    },
    [model, onModelChange, mapping],
  );
}

export function AudioGraphCanvas({
  addNodeLabel,
  contextMenuLabels,
  ariaLabel,
  meterStore,
  model,
  nodeMenuItems = [],
  onAddNode,
  onModelChange,
  onNodeMenuOpenedChange,
  onNodeRenameRequest,
  nodeContextActions,
  onProcessorEditorRetry,
  processorEditorLabels,
  processorEditorStatuses = emptyProcessorEditorStatuses,
  renderNodeContent,
  renderNodeControls,
  viewport,
  onViewportChange,
  ...edits
}: AudioGraphCanvasProps) {
  const options = useState<string | null>(null);
  const disclosureHeights = useRef(new Map<string, number>()).current;
  const nodeMenu = useNodeMenu(onNodeMenuOpenedChange);
  const viewportEvents = useCanvasViewport(viewport, onViewportChange, nodeMenu);
  const commit = useGraphCommit(model, onModelChange);
  const flow = useAudioFlowCollections(model, commit, disclosureHeights, options[0]);
  const portSelection = useAudioPortSelection(nodeMenu.rootRef, flow.selectEdges);
  const connections = useConnectionHandlers(
    model,
    commit,
    portSelection.selectedRef,
    portSelection.clear,
  );
  const deletions = useDeletionHandlers(model, commit);
  const { addNode, onPaneClick } = useCanvasViewActions(
    onAddNode,
    nodeMenu,
    portSelection.clear,
    onViewportChange,
  );
  // A new context object each render would re-render every node.
  const editorContext = useMemo(
    () =>
      nodeProcessorEditors(onProcessorEditorRetry, processorEditorLabels, processorEditorStatuses),
    [onProcessorEditorRetry, processorEditorLabels, processorEditorStatuses],
  );
  const { renaming, connecting } = useNodeRequests(onNodeRenameRequest, nodeMenu.rootRef);
  const contextMenu = useGraphContextMenu({
    model,
    nodes: flow.nodes,
    edges: flow.edges,
    setNodes: flow.setNodes,
    setEdges: flow.setEdges,
    addItems: nodeMenuItems,
    labels: contextMenuLabels,
    renameNode: renaming.request,
    nodeActions: nodeContextActions,
    connectChannels: connecting.request,
    commit,
    clearPorts: portSelection.clear,
    selectPorts: portSelection.select,
    selectedPorts: portSelection.selectedRef,
    fitView: () => void nodeMenu.flowRef.current?.fitView(fitViewOptions),
  });

  return (
    <>
      <GraphContextSurface
        contextMenu={contextMenu}
        nodeMenu={nodeMenu}
        addNode={addNode}
        ariaLabel={ariaLabel}
        captureProps={portSelection.captureProps}
        renameNode={renaming.request}
      >
        <PortSelectionFeedback model={model} {...portSelection.feedback} />
        <AudioGraphProviders
          options={options}
          editor={editorContext}
          meterStore={meterStore ?? null}
          model={model}
          edits={edits}
          selectedRef={portSelection.selectedRef}
          renderNodeContent={renderNodeContent}
          renderNodeControls={renderNodeControls}
          disclosureHeights={disclosureHeights}
        >
          <GraphFlowSurface
            {...viewportEvents}
            {...connections}
            {...deletions}
            focusRoot={nodeMenu.rootRef}
            defaultViewport={viewport}
            edges={flow.edges}
            fitView={viewport === undefined}
            nodes={flow.nodes}
            onNodeDoubleClick={renaming.doubleClick}
            onEdgesChange={flow.applyEdgeChanges}
            onNodesChange={flow.onNodesChange}
            onPaneClick={() => {
              options[1](null);
              onPaneClick();
            }}
          >
            <GraphCanvasBackground
              addNode={addNode}
              empty={flow.nodes.length === 0}
              label={addNodeLabel}
              nodeMenu={nodeMenu}
              nodeMenuItems={nodeMenuItems}
            />
          </GraphFlowSurface>
        </AudioGraphProviders>
      </GraphContextSurface>
      <CanvasConnectionDialog connecting={connecting} model={model} commit={commit} />
    </>
  );
}

function CanvasConnectionDialog({
  connecting,
  model,
  commit,
}: {
  connecting: ReturnType<typeof useChannelConnectionRequests>;
  model: AudioGraphModel;
  commit: CommitGraph;
}) {
  if (!connecting.current) return null;
  return (
    <ChannelConnectionDialog
      key={connecting.current.nodeId}
      model={model}
      sourceNodeId={connecting.current.nodeId}
      sourcePortIds={connecting.current.portIds}
      onCancel={connecting.close}
      onConnect={(connections) => {
        // One commit, so the whole batch is a single undo step.
        commit(connectGraphBatch(model, connections), 'runtime');
        connecting.close();
      }}
    />
  );
}
