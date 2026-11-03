import type { ProcessorEditorFailure, ProcessorEditorStatus } from '../domain/control/processors';
import { planChannelConnections } from '../domain/graph/channelConnections';
import {
  canConnectGraph,
  connectGraphBatch,
  disconnectGraph,
  type GraphConnection,
  reconnectGraph,
  removeGraphItems,
} from '../domain/graph/topology';
import { MidiContext, MidiNodeContext } from '../midi/MidiContext';
import { useStructuralShare } from '../ui/useStructuralShare';
import { type AudioGraphEdgeEndpoints, namedAudioPorts } from './portNames';
import type { SelectedAudioPort } from './portSelection';
import '@xyflow/react/dist/base.css';

import {
  type AriaLabelConfig,
  Background,
  BackgroundVariant,
  type Connection,
  type ConnectionLineComponentProps,
  ConnectionMode,
  type Edge,
  getBezierPath,
  Handle,
  type NodeChange,
  type NodeProps,
  NodeResizeControl,
  Position,
  ReactFlow,
  type ReactFlowInstance,
  Panel as ReactFlowPanel,
  type ReactFlowProps,
  useConnection,
  useEdgesState,
  useNodesState,
  useUpdateNodeInternals,
  type XYPosition,
} from '@xyflow/react';
import {
  createContext,
  type Dispatch,
  memo,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  type RefObject,
  type SetStateAction,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';
import { AudioMeterProvider } from '../audio-meter/AudioMeterProvider';
import type { AudioMeterStore } from '../audio-meter/AudioMeterStore';
import { AudioNodeCard } from '../ui/AudioNodeCard';
import type { EndpointUpdate } from '../ui/AudioNodeEndpointOptions';
import { audioNodePortTop } from '../ui/audioNodeLayout';
import {
  HierarchicalContextMenu,
  HierarchicalMenuButton,
  type HierarchicalMenuItem,
} from '../ui/HierarchicalMenu';
import {
  NodeOptionsContext,
  NodeOptionsIdContext,
  type NodeOptionsState,
} from '../ui/nodeOptionsState';
import { platformModifier } from '../ui/platformShortcut';
import { Tooltip } from '../ui/Tooltip';
import styles from './AudioGraphCanvas.module.css';
import { ChannelConnectionDialog } from './ChannelConnectionDialog';
import { connectionPreviewPairs, dragConnectionRequest } from './connectionPlanning';
import {
  focusGraphNode,
  graphContextMenuAnchor,
  openContextMenuFromKeyboard,
  useChannelConnectionRequests,
} from './graphKeyboard';
import { type AudioGraphViewport, viewportToRestore, viewportToSave } from './graphViewport';
import {
  type AudioGraphModel,
  type AudioGraphNode,
  type AudioGraphPort,
  audioGraphNodeTitle,
  graphNodeMinimumWidth,
  normalizeAudioGraphNodeSize,
} from './model';
import { applyNodeLayoutChanges } from './nodeChanges';
import type { PlaceGraphNodes } from './nodePlacement';
import { PortSelectionFeedback } from './PortSelectionFeedback';
import {
  type AudioFlowEdge,
  type AudioFlowEdgeLabel,
  type AudioFlowNode,
  applyAudioNodeLayers,
  reconcileAudioFlowEdges,
  reconcileAudioFlowNodes,
} from './reactFlowAdapter';
import { graphNodeFaceMode, useAudioNodeLayout } from './useAudioNodeLayout';
import { useAudioPortSelection } from './useAudioPortSelection';
import {
  type GraphContextMenuLabels,
  type NodeContextAction,
  useGraphContextMenu,
} from './useGraphContextMenu';
import { useNodeInsertion } from './useNodeInsertion';

const nodeTypes = { audio: memo(AudioFlowNodeView) };
const deleteKeyCode = ['Backspace', 'Delete'];
const fitViewOptions = { padding: 0.15, maxZoom: 1 };
const panMouseButtons = [1];
const snapGrid: [number, number] = [12, 12];
const emptyProcessorEditorStatuses = new Map<string, ProcessorEditorStatus>();
const NodeGainContext = createContext<((nodeId: string, gainDb: number) => void) | undefined>(
  undefined,
);
/** Node settings edited from Options; one memoized value so nodes skip unchanged renders. */
const NodeSettingsContext = createContext<{
  endpoint?: AudioGraphCanvasProps['onNodeEndpointChange'];
  channelLayout?: AudioGraphCanvasProps['onNodeChannelLayoutChange'];
}>({});
const NodeParameterContext = createContext<{
  single?: (nodeId: string, parameterId: string, value: number) => void;
  batch?: (nodeId: string, updates: readonly { id: string; value: number }[]) => void;
}>({});
export interface ProcessorEditorLabels {
  open: string;
  retry: string;
  failure: Record<ProcessorEditorFailure, string>;
}
interface NodeProcessorEditorContextValue {
  labels: ProcessorEditorLabels;
  onRetry: (nodeId: string) => void;
  statuses: ReadonlyMap<string, ProcessorEditorStatus>;
}
const NodeProcessorEditorContext = createContext<NodeProcessorEditorContextValue | null>(null);
const NodeContentContext = createContext<((node: AudioGraphNode) => ReactNode) | undefined>(
  undefined,
);
export interface AudioNodeControlSlots {
  power?: ReactNode;
  mute?: ReactNode;
}
const NodeControlsContext = createContext<
  ((node: AudioGraphNode) => AudioNodeControlSlots | null) | undefined
>(undefined);
export type AudioGraphChangeKind =
  | 'layout'
  | 'level'
  | 'processor_parameter'
  | 'parameter_group'
  | 'runtime';
type CommitGraph = (nextModel: AudioGraphModel, kind: AudioGraphChangeKind) => void;
type ReconnectEnd = NonNullable<ReactFlowProps<AudioFlowNode, AudioFlowEdge>['onReconnectEnd']>;

type AudioPortSide = 'input' | 'output';

interface AudioPortSelectionContextValue {
  model: AudioGraphModel;
  selectedRef: { current: SelectedAudioPort[] };
}

const AudioPortSelectionContext = createContext<AudioPortSelectionContextValue | null>(null);
const NodeDisclosureHeightsContext = createContext<Map<string, number> | null>(null);

export interface AudioGraphCanvasProps {
  nodeContextActions?: (nodeId: string) => readonly NodeContextAction[];
  renderNodeControls?: (node: AudioGraphNode) => AudioNodeControlSlots | null;
  renderNodeContent?: (node: AudioGraphNode) => ReactNode;
  contextMenuLabels?: GraphContextMenuLabels;
  addNodeLabel?: string;
  ariaLabel: string;
  meterStore?: AudioMeterStore;
  model: AudioGraphModel;
  nodeMenuItems?: readonly AudioGraphNodeMenuItem[];
  onAddNode?: (itemId: string, position: XYPosition, placeNodes?: PlaceGraphNodes) => void;
  onNodeMenuOpenedChange?: (opened: boolean) => void;
  onNodeRenameRequest?: (nodeId: string) => void;
  onModelChange: (model: AudioGraphModel, kind: AudioGraphChangeKind) => void;
  onNodeEndpointChange?: (nodeId: string, value: EndpointUpdate) => void;
  /** A choice from the node's `channelLayoutControl`. */
  onNodeChannelLayoutChange?: (nodeId: string, value: string) => void;
  onNodeGainChange?: (nodeId: string, gainDb: number) => void;
  onNodeParameterChange?: (nodeId: string, parameterId: string, value: number) => void;
  onNodeParametersChange?: (
    nodeId: string,
    updates: readonly { id: string; value: number }[],
  ) => void;
  onProcessorEditorRetry?: (nodeId: string) => void;
  processorEditorLabels?: ProcessorEditorLabels;
  processorEditorStatuses?: ReadonlyMap<string, ProcessorEditorStatus>;
  /** Initial or externally restored view; user movement remains locally interactive. */
  viewport?: AudioGraphViewport;
  /** Reports user pan/zoom and insertion reveals, never external viewport restoration. */
  onViewportChange?: (viewport: AudioGraphViewport) => void;
}

export type AudioGraphNodeMenuItem = HierarchicalMenuItem;

/** Per-node edits a node card reports from its controls and Options. */
type NodeEdits = Pick<
  AudioGraphCanvasProps,
  | 'onNodeEndpointChange'
  | 'onNodeChannelLayoutChange'
  | 'onNodeGainChange'
  | 'onNodeParameterChange'
  | 'onNodeParametersChange'
>;

function AudioGraphProviders({
  children,
  options,
  editor,
  meterStore,
  model,
  edits: {
    onNodeEndpointChange: onEndpointChange,
    onNodeChannelLayoutChange: onChannelLayoutChange,
    onNodeGainChange: onGainChange,
    onNodeParameterChange: onParameterChange,
    onNodeParametersChange: onParametersChange,
  },
  selectedRef,
  renderNodeContent,
  renderNodeControls,
  disclosureHeights,
}: {
  children: ReactNode;
  options: NodeOptionsState;
  editor: NodeProcessorEditorContextValue | null;
  meterStore: AudioMeterStore | null;
  model: AudioGraphModel;
  edits: NodeEdits;
  selectedRef: AudioPortSelectionContextValue['selectedRef'];
  renderNodeContent: AudioGraphCanvasProps['renderNodeContent'];
  renderNodeControls: AudioGraphCanvasProps['renderNodeControls'];
  disclosureHeights: Map<string, number>;
}) {
  // Context values are memoized: a new object would re-render every node.
  const parameters = useMemo(
    () => ({ single: onParameterChange, batch: onParametersChange }),
    [onParameterChange, onParametersChange],
  );
  const settings = useMemo(
    () => ({ endpoint: onEndpointChange, channelLayout: onChannelLayoutChange }),
    [onEndpointChange, onChannelLayoutChange],
  );
  const selection = useMemo(() => ({ model, selectedRef }), [model, selectedRef]);
  // `useState` returns a new tuple every render; keep the context value stable.
  const [openedOptions, setOpenedOptions] = options;
  const optionsValue = useMemo<NodeOptionsState>(
    () => [openedOptions, setOpenedOptions],
    [openedOptions, setOpenedOptions],
  );
  return (
    <AudioMeterProvider store={meterStore}>
      <NodeProcessorEditorContext.Provider value={editor}>
        <NodeParameterContext.Provider value={parameters}>
          <NodeSettingsContext.Provider value={settings}>
            <NodeGainContext.Provider value={onGainChange}>
              <AudioPortSelectionContext.Provider value={selection}>
                <NodeContentContext.Provider value={renderNodeContent}>
                  <NodeControlsContext.Provider value={renderNodeControls}>
                    <NodeOptionsContext.Provider value={optionsValue}>
                      <NodeDisclosureHeightsContext.Provider value={disclosureHeights}>
                        {children}
                      </NodeDisclosureHeightsContext.Provider>
                    </NodeOptionsContext.Provider>
                  </NodeControlsContext.Provider>
                </NodeContentContext.Provider>
              </AudioPortSelectionContext.Provider>
            </NodeGainContext.Provider>
          </NodeSettingsContext.Provider>
        </NodeParameterContext.Provider>
      </NodeProcessorEditorContext.Provider>
    </AudioMeterProvider>
  );
}

function processorEditorContext(
  onRetry: AudioGraphCanvasProps['onProcessorEditorRetry'],
  labels: ProcessorEditorLabels | undefined,
  statuses: ReadonlyMap<string, ProcessorEditorStatus>,
): NodeProcessorEditorContextValue | null {
  return onRetry && labels ? { labels, onRetry, statuses } : null;
}

function useCanvasViewActions(
  onAddNode: AudioGraphCanvasProps['onAddNode'],
  onProcessorEditorRetry: AudioGraphCanvasProps['onProcessorEditorRetry'],
  processorEditorLabels: ProcessorEditorLabels | undefined,
  processorEditorStatuses: ReadonlyMap<string, ProcessorEditorStatus>,
  nodeMenu: ReturnType<typeof useNodeMenu>,
  clearPortSelection: () => void,
  onReveal: AudioGraphCanvasProps['onViewportChange'],
) {
  const placeNodes = useNodeInsertion(nodeMenu, onReveal);
  const addNode = (itemId: string) => onAddNode?.(itemId, nodeMenu.positionRef.current, placeNodes);
  const onPaneClick = useCallback(() => {
    nodeMenu.close();
    clearPortSelection();
  }, [clearPortSelection, nodeMenu.close]);
  // A new context object each render would re-render every node.
  const editorContext = useMemo(
    () =>
      processorEditorContext(
        onProcessorEditorRetry,
        processorEditorLabels,
        processorEditorStatuses,
      ),
    [onProcessorEditorRetry, processorEditorLabels, processorEditorStatuses],
  );
  return { addNode, editorContext, onPaneClick };
}

function useNodeMenu(onOpenedChange?: (opened: boolean) => void) {
  const rootRef = useRef<HTMLElement | null>(null);
  const flowRef = useRef<ReactFlowInstance<AudioFlowNode, AudioFlowEdge> | null>(null);
  const positionRef = useRef<XYPosition>({ x: 0, y: 0 });
  const [buttonOpened, setButtonOpened] = useState(false);
  const [contextOpened, setContextOpened] = useState(false);
  const opened = buttonOpened || contextOpened;
  useEffect(() => onOpenedChange?.(opened), [opened, onOpenedChange]);
  const close = useCallback(() => {
    setButtonOpened(false);
    setContextOpened(false);
  }, []);
  const setFlowInstance = useCallback(
    (instance: ReactFlowInstance<AudioFlowNode, AudioFlowEdge>) => {
      flowRef.current = instance;
    },
    [],
  );
  const prepareContextPosition = useCallback((event: { clientX: number; clientY: number }) => {
    setButtonOpened(false);
    // These handlers run from the mounted canvas and its child trigger.
    const bounds = (rootRef.current as HTMLElement).getBoundingClientRect();
    const screenPosition = {
      x: event.clientX - bounds.left,
      y: event.clientY - bounds.top,
    };
    positionRef.current =
      flowRef.current?.screenToFlowPosition({ x: event.clientX, y: event.clientY }) ??
      screenPosition;
  }, []);
  // The menu opens on press start, before any click event reaches the trigger, so the
  // anchor is taken when the menu opens, for pointer and keyboard presses alike.
  const openButtonMenu = useCallback((next: boolean) => {
    setButtonOpened(next);
    if (!next) return;
    setContextOpened(false);
    // The trigger renders inside the mounted canvas.
    const bounds = (rootRef.current as HTMLElement).getBoundingClientRect();
    const screenPosition = { x: 132, y: 92 };
    positionRef.current =
      flowRef.current?.screenToFlowPosition({
        x: bounds.left + screenPosition.x,
        y: bounds.top + screenPosition.y,
      }) ?? screenPosition;
  }, []);
  return {
    buttonOpened,
    close,
    contextOpened,
    flowRef,
    openButtonMenu,
    positionRef,
    prepareContextPosition,
    rootRef,
    setContextOpened,
    setFlowInstance,
  };
}

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

/** React Flow's built-in keyboard and live-region text follows the app language. */
function useGraphAriaLabels(): Partial<AriaLabelConfig> {
  const { t } = useTranslation();
  return useMemo(
    () => ({
      'node.a11yDescription.default': t('routing.a11y.nodeDescriptionStatic'),
      'node.a11yDescription.keyboardDisabled': t('routing.a11y.nodeDescription'),
      'node.a11yDescription.ariaLiveMessage': ({ direction, x, y }) =>
        t('routing.a11y.nodeMoved', {
          direction: t(`routing.a11y.directions.${direction as ArrowDirection}`),
          x,
          y,
        }),
      'edge.a11yDescription.default': t('routing.a11y.edgeDescription'),
    }),
    [t],
  );
}

type ArrowDirection = 'up' | 'down' | 'left' | 'right';

function useAudioFlowCollections(
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
  const onNodesChange = useNodeChangeHandler(model, commit, applyNodeChanges, disclosureHeights);
  const layeredNodes = useMemo(
    () => applyAudioNodeLayers(nodes, openedNodeId),
    [nodes, openedNodeId],
  );
  return { applyEdgeChanges, onNodesChange, edges, nodes: layeredNodes, setEdges, setNodes };
}

function toGraphConnection(connection: Connection | Edge): GraphConnection {
  return {
    sourceNodeId: connection.source,
    sourcePortId: connection.sourceHandle ?? '',
    targetNodeId: connection.target,
    targetPortId: connection.targetHandle ?? '',
  };
}

function portVerticalOffset(
  node: ConnectionLineComponentProps<AudioFlowNode>['fromNode'],
  side: AudioPortSide,
  portId: string,
  anchorPortId: string,
) {
  const handles = node.internals.handleBounds?.[side === 'input' ? 'target' : 'source'];
  const port = handles?.find((handle) => handle.id === portId);
  const anchor = handles?.find((handle) => handle.id === anchorPortId);
  if (!port || !anchor) return 0;
  // React Flow applies viewport zoom to the whole connection layer. Use measured
  // handle centers in node coordinates, including the node's current resize.
  return port.y + port.height / 2 - anchor.y - anchor.height / 2;
}

function BatchConnectionLine({
  connectionLineStyle,
  connectionStatus,
  fromHandle,
  fromNode,
  fromPosition,
  fromX,
  fromY,
  toHandle,
  toNode,
  toPosition,
  toX,
  toY,
}: ConnectionLineComponentProps<AudioFlowNode>) {
  // This private renderer is installed only below AudioGraphProviders.
  const selection = useContext(AudioPortSelectionContext) as AudioPortSelectionContextValue;
  const sourceAnchorId = fromHandle.id ?? '';
  const targetAnchorId = toHandle?.id ?? '';
  const previews = connectionPreviewPairs(
    selection.model,
    selection.selectedRef.current,
    fromNode.id,
    sourceAnchorId,
    toNode?.id,
    targetAnchorId,
  );

  return (
    <>
      {previews.map(({ sourcePort, targetPort }, index) => {
        const sourceOffset = portVerticalOffset(fromNode, 'output', sourcePort.id, sourceAnchorId);
        const targetOffset =
          toNode && targetPort
            ? portVerticalOffset(toNode, 'input', targetPort.id, targetAnchorId)
            : 0;
        const [path] = getBezierPath({
          sourcePosition: fromPosition,
          sourceX: fromX,
          sourceY: fromY + sourceOffset,
          targetPosition: toPosition,
          targetX: toX,
          targetY: toY + targetOffset,
        });
        return (
          <path
            className="react-flow__connection-path"
            data-connection-status={connectionStatus ?? undefined}
            d={path}
            fill="none"
            key={`${sourcePort.id}:${targetPort?.id ?? index}`}
            style={connectionLineStyle}
          />
        );
      })}
    </>
  );
}

function useConnectionHandlers(
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

function useDeletionHandlers(model: AudioGraphModel, commit: CommitGraph) {
  const removeEdges = useCallback(
    (removedEdges: AudioFlowEdge[]) =>
      commit(
        disconnectGraph(
          model,
          removedEdges.map((edge) => edge.id),
        ),
        'runtime',
      ),
    [commit, model],
  );
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
    (_: ReactMouseEvent, edge: AudioFlowEdge) => removeEdges([edge]),
    [removeEdges],
  );
  return { onDelete, onEdgeDoubleClick };
}

function useNodeChangeHandler(
  model: AudioGraphModel,
  commit: CommitGraph,
  applyNodeChanges: (changes: NodeChange<AudioFlowNode>[]) => void,
  disclosureHeights: Map<string, number>,
) {
  return useCallback(
    (changes: NodeChange<AudioFlowNode>[]) => {
      // Preserve XYFlow's live measurements, but do not pin the outer height:
      // the card includes temporary Options content in its rendered bounds.
      applyNodeChanges(
        changes.map((change) =>
          change.type === 'dimensions' && change.setAttributes
            ? { ...change, setAttributes: 'width' }
            : change,
        ),
      );
      commit(applyNodeLayoutChanges(model, changes, disclosureHeights), 'layout');
    },
    [applyNodeChanges, commit, disclosureHeights, model],
  );
}

function AddNodePanel({
  addNode,
  label,
  nodeMenu,
  nodeMenuItems,
}: {
  addNode: (itemId: string) => void;
  label?: string;
  nodeMenu: ReturnType<typeof useNodeMenu>;
  nodeMenuItems: readonly AudioGraphNodeMenuItem[];
}) {
  const { t } = useTranslation();
  const actionLabel = label?.trim() || t('routing.addNode');
  const mapping = useContext(MidiContext)?.active ?? false;
  if (nodeMenuItems.length === 0 || mapping) return null;
  return (
    <ReactFlowPanel className={`${styles.nodeToolbar} nodrag nopan nowheel`} position="top-left">
      <HierarchicalMenuButton
        buttonClassName={`${styles.addNode} nodrag nopan nowheel`}
        buttonContent={<span aria-hidden="true">+</span>}
        items={nodeMenuItems}
        label={actionLabel}
        tooltip={actionLabel}
        onOpenedChange={nodeMenu.openButtonMenu}
        onSelect={addNode}
        opened={nodeMenu.buttonOpened}
      />
    </ReactFlowPanel>
  );
}

function GraphCanvasBackground({
  addNode,
  empty,
  label,
  nodeMenu,
  nodeMenuItems,
}: Parameters<typeof AddNodePanel>[0] & { empty: boolean }) {
  const { t } = useTranslation();
  return (
    <>
      <Background color="var(--canvas-grid)" gap={24} size={1} variant={BackgroundVariant.Lines} />
      {/* Guidance only: the + panel stays the one owner of the add-node action. */}
      {empty && nodeMenuItems.length > 0 ? (
        <p className={styles.emptyHint}>{t('routing.emptyCanvas')}</p>
      ) : null}
      <AddNodePanel
        addNode={addNode}
        label={label}
        nodeMenu={nodeMenu}
        nodeMenuItems={nodeMenuItems}
      />
    </>
  );
}

function useEdgeBoxSelection(setEdges: Dispatch<SetStateAction<AudioFlowEdge[]>>) {
  return useCallback(
    (edgeIds: readonly string[]) => {
      if (edgeIds.length === 0) return;
      const selectedIds = new Set(edgeIds);
      requestAnimationFrame(() => {
        setEdges((current) =>
          current.map((edge) =>
            selectedIds.has(edge.id) && !edge.selected ? { ...edge, selected: true } : edge,
          ),
        );
      });
    },
    [setEdges],
  );
}

function handleGraphKeyDown(
  event: ReactKeyboardEvent<HTMLElement>,
  captureProps: ReturnType<typeof useAudioPortSelection>['captureProps'],
  clearSelection: () => void,
  renameNode?: (nodeId: string) => void,
) {
  if (openContextMenuFromKeyboard(event)) return;
  if (event.key === 'F2' && event.target instanceof HTMLElement && renameNode) {
    const id = event.target.matches('.react-flow__node') ? event.target.dataset.id : undefined;
    if (id) {
      event.preventDefault();
      event.stopPropagation();
      renameNode(id);
      return;
    }
  }
  captureProps.onKeyDownCapture(event);
  if (
    event.key === 'Escape' &&
    event.target instanceof Element &&
    event.target.matches('.react-flow__node, .react-flow__edge')
  ) {
    // React Flow blurs on Escape. Clear selection while keeping the user's place.
    event.preventDefault();
    event.stopPropagation();
    clearSelection();
  }
}

function GraphContextSurface({
  children,
  contextMenu,
  nodeMenu,
  addNode,
  ariaLabel,
  captureProps,
  renameNode,
}: {
  children: ReactNode;
  contextMenu: ReturnType<typeof useGraphContextMenu>;
  nodeMenu: ReturnType<typeof useNodeMenu>;
  addNode: (id: string) => void;
  ariaLabel: string;
  captureProps: ReturnType<typeof useAudioPortSelection>['captureProps'];
  renameNode?: (nodeId: string) => void;
}) {
  const mapping = useContext(MidiContext)?.active ?? false;
  return (
    <HierarchicalContextMenu
      getAnchor={graphContextMenuAnchor}
      items={mapping ? [] : contextMenu.items}
      onOpenedChange={nodeMenu.setContextOpened}
      onSelect={(id) => {
        nodeMenu.close();
        if (!contextMenu.execute(id)) addNode(id);
      }}
      opened={nodeMenu.contextOpened && !mapping}
    >
      <section
        {...captureProps}
        onKeyDownCapture={(event) =>
          !mapping &&
          handleGraphKeyDown(event, captureProps, contextMenu.clearSelection, renameNode)
        }
        onPointerDownCapture={(event) => {
          // The whole canvas is the context trigger, so the menu's outside-click
          // handler cannot dismiss clicks on other nodes or ports inside it.
          if (event.button !== 2) nodeMenu.setContextOpened(false);
          if (!mapping) captureProps.onPointerDownCapture(event);
        }}
        aria-label={ariaLabel}
        tabIndex={-1}
        className={styles.root}
        data-midi-mode={mapping || undefined}
        ref={nodeMenu.rootRef}
        onContextMenu={(event) => {
          contextMenu.prepare(event);
          const point = graphContextMenuAnchor(event);
          nodeMenu.prepareContextPosition({ clientX: point.x, clientY: point.y });
          // React normalizes mouse events on text nodes to their parent Element.
          const owner = (event.target as Element).closest<HTMLElement>(
            '.react-flow__node, .react-flow__edge',
          );
          owner?.focus();
        }}
        onTouchStart={(event) => {
          // the context trigger's long-press opens without dispatching a contextmenu event.
          // Capture the target and position for that path as well.
          const touch = event.touches[0];
          if (!touch) return;
          contextMenu.prepare(event);
          nodeMenu.prepareContextPosition(touch);
        }}
        onContextMenuCapture={(event) => {
          // Native text editing menus must not act on the enclosing node.
          if (
            event.target instanceof Element &&
            event.target.closest('input, textarea, [contenteditable="true"]')
          )
            event.stopPropagation();
        }}
      >
        {children}
      </section>
    </HierarchicalContextMenu>
  );
}

function restoreCanvasViewport(
  instance: ReactFlowInstance<AudioFlowNode, AudioFlowEdge> | null,
  viewport: AudioGraphViewport | undefined,
) {
  const restored = instance && viewportToRestore(instance.getViewport(), viewport);
  if (restored) void instance.setViewport(restored, { duration: 0 });
}

function useCanvasViewport(
  viewport: AudioGraphViewport | undefined,
  onViewportChange: AudioGraphCanvasProps['onViewportChange'],
  nodeMenu: Pick<ReturnType<typeof useNodeMenu>, 'flowRef' | 'setFlowInstance'>,
) {
  const { flowRef, setFlowInstance } = nodeMenu;
  const mounted = useRef(true);
  const requestedViewport = useRef(viewport);
  requestedViewport.current = viewport;
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const onInit = useCallback(
    (instance: ReactFlowInstance<AudioFlowNode, AudioFlowEdge>) => {
      if (!mounted.current) return;
      setFlowInstance(instance);
      // React Flow can finish initializing after the external document has changed.
      restoreCanvasViewport(instance, requestedViewport.current);
    },
    [setFlowInstance],
  );
  const x = viewport?.x;
  const y = viewport?.y;
  const zoom = viewport?.zoom;
  useEffect(() => {
    if (x !== undefined && y !== undefined && zoom !== undefined)
      restoreCanvasViewport(flowRef.current, { x, y, zoom });
  }, [flowRef, x, y, zoom]);
  const onMoveEnd = useCallback<
    NonNullable<ReactFlowProps<AudioFlowNode, AudioFlowEdge>['onMoveEnd']>
  >(
    (_, nextViewport) => {
      // onMoveEnd reports once per gesture; restoring the requested view echoes it back.
      const saved = mounted.current && viewportToSave(requestedViewport.current, nextViewport);
      if (saved) onViewportChange?.(saved);
    },
    [onViewportChange],
  );
  return { onInit, onMoveEnd };
}

function GraphFlowSurface({
  focusRoot,
  ...props
}: ReactFlowProps<AudioFlowNode, AudioFlowEdge> & { focusRoot: RefObject<HTMLElement | null> }) {
  const ariaLabelConfig = useGraphAriaLabels();
  const mapping = useContext(MidiContext)?.active ?? false;
  // React Flow's deletion keys listen on the whole document. Keeping the listener mounted
  // preserves its key tracking; deletion itself proceeds only while focus is in the graph,
  // so Delete in a dialog or elsewhere never removes the selection behind it.
  const onBeforeDelete = useCallback(
    () => Promise.resolve(Boolean(!mapping && focusRoot.current?.contains(document.activeElement))),
    [focusRoot, mapping],
  );
  return (
    <ReactFlow<AudioFlowNode, AudioFlowEdge>
      {...props}
      ariaLabelConfig={ariaLabelConfig}
      colorMode="dark"
      proOptions={{ hideAttribution: true }}
      connectionLineComponent={BatchConnectionLine}
      connectionMode={ConnectionMode.Strict}
      zIndexMode="manual"
      deleteKeyCode={deleteKeyCode}
      onBeforeDelete={onBeforeDelete}
      onNodeClick={mapping ? (event) => event.stopPropagation() : props.onNodeClick}
      onNodeDoubleClick={mapping ? undefined : props.onNodeDoubleClick}
      nodesDraggable={!mapping}
      nodesConnectable={!mapping}
      nodesFocusable={!mapping}
      edgesFocusable={!mapping}
      elementsSelectable={!mapping}
      disableKeyboardA11y={mapping}
      edgesReconnectable={!mapping}
      fitViewOptions={fitViewOptions}
      // Match the persisted layout contract; React Flow's defaults would clamp restored views.
      minZoom={0.05}
      maxZoom={8}
      multiSelectionKeyCode="Shift"
      nodeTypes={nodeTypes}
      panActivationKeyCode={mapping ? null : platformModifier()}
      panOnDrag={mapping ? false : panMouseButtons}
      zoomOnScroll={!mapping}
      zoomOnPinch={!mapping}
      zoomOnDoubleClick={!mapping}
      reconnectRadius={18}
      selectionOnDrag={!mapping}
      snapGrid={snapGrid}
      snapToGrid
    />
  );
}

function useGraphCommit(
  model: AudioGraphModel,
  onModelChange: AudioGraphCanvasProps['onModelChange'],
) {
  const mapping = useContext(MidiContext)?.active ?? false;
  return useCallback(
    (nextModel: AudioGraphModel, kind: AudioGraphChangeKind) => {
      if (!mapping && nextModel !== model) onModelChange(nextModel, kind);
    },
    [model, onModelChange, mapping],
  );
}

function clearPaneOptions(clear: (value: string | null) => void, click: () => void) {
  clear(null);
  click();
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
  const { applyEdgeChanges, onNodesChange, edges, nodes, setEdges, setNodes } =
    useAudioFlowCollections(model, commit, disclosureHeights, options[0]);
  const selectEdges = useEdgeBoxSelection(setEdges);
  const portSelection = useAudioPortSelection(nodeMenu.rootRef, selectEdges);
  const { isValidConnection, onConnect, onReconnect, onReconnectEnd } = useConnectionHandlers(
    model,
    commit,
    portSelection.selectedRef,
    portSelection.clear,
  );
  const { onDelete, onEdgeDoubleClick } = useDeletionHandlers(model, commit);
  const { addNode, editorContext, onPaneClick } = useCanvasViewActions(
    onAddNode,
    onProcessorEditorRetry,
    processorEditorLabels,
    processorEditorStatuses,
    nodeMenu,
    portSelection.clear,
    onViewportChange,
  );
  const { renaming, connecting } = useNodeRequests(onNodeRenameRequest, nodeMenu.rootRef);
  const contextMenu = useGraphContextMenu({
    ...{ model, nodes, edges, setNodes, setEdges },
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
            focusRoot={nodeMenu.rootRef}
            defaultViewport={viewport}
            edges={edges}
            fitView={viewport === undefined}
            isValidConnection={isValidConnection}
            nodes={nodes}
            onConnect={onConnect}
            onDelete={onDelete}
            onEdgeDoubleClick={onEdgeDoubleClick}
            onNodeDoubleClick={renaming.doubleClick}
            onEdgesChange={applyEdgeChanges}
            onNodesChange={onNodesChange}
            onPaneClick={() => clearPaneOptions(options[1], onPaneClick)}
            onReconnect={onReconnect}
            onReconnectEnd={onReconnectEnd}
          >
            <GraphCanvasBackground
              addNode={addNode}
              empty={nodes.length === 0}
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
  commit: ReturnType<typeof useGraphCommit>;
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

/** Node-scoped dialogs share one focus contract: the node's outer group owns return focus. */
function useNodeRequests(
  onRename: AudioGraphCanvasProps['onNodeRenameRequest'],
  root: { current: HTMLElement | null },
) {
  return {
    renaming: useNodeRenameRequests(onRename, root),
    connecting: useChannelConnectionRequests(root),
  };
}

function useNodeRenameRequests(
  onRequest: AudioGraphCanvasProps['onNodeRenameRequest'],
  root: { current: HTMLElement | null },
) {
  const request = useMemo(() => {
    if (!onRequest) return undefined;
    return (id: string) => {
      focusGraphNode(root.current as HTMLElement, id);
      onRequest(id);
    };
  }, [root, onRequest]);
  return {
    request,
    doubleClick: request
      ? (event: ReactMouseEvent, node: AudioFlowNode) => renameNodeTitle(event, node.id, request)
      : undefined,
  };
}

function renameNodeTitle(event: ReactMouseEvent, id: string, rename: (nodeId: string) => void) {
  if (!(event.target instanceof Element) || !event.target.closest('[data-node-title]')) return;
  event.preventDefault();
  event.stopPropagation();
  event.target.closest<HTMLElement>('.react-flow__node')?.focus();
  rename(id);
}

function AudioFlowNodeView({ data, selected }: NodeProps<AudioFlowNode>) {
  const { t } = useTranslation();
  const { graphNode } = data;
  // AudioFlowNodeView is private to the canvas provider tree.
  const disclosureHeights = useContext(NodeDisclosureHeightsContext) as Map<string, number>;
  const onNodeGainChange = useContext(NodeGainContext);
  const settings = useContext(NodeSettingsContext);
  const { single: onNodeParameterChange, batch: onNodeParametersChange } =
    useContext(NodeParameterContext);
  const processorEditors = useContext(NodeProcessorEditorContext);
  const renderNodeContent = useContext(NodeContentContext);
  const controls = useContext(NodeControlsContext)?.(graphNode);
  const updateNodeInternals = useUpdateNodeInternals();
  const editorStatus = processorEditors?.statuses.get(graphNode.id);
  const editorWidth = editorStatus?.state === 'open' ? editorStatus.width : null;
  const layout = useAudioNodeLayout(graphNode, {
    width: editorWidth,
    height: editorStatus?.height ?? null,
  });
  useLayoutEffect(() => {
    if (layout.disclosureHeight > 0) disclosureHeights.set(graphNode.id, layout.disclosureHeight);
    else disclosureHeights.delete(graphNode.id);
    return () => {
      disclosureHeights.delete(graphNode.id);
    };
  }, [disclosureHeights, graphNode.id, layout.disclosureHeight]);
  return (
    <MidiNodeScope node={graphNode}>
      <NodeOptionsIdContext.Provider value={graphNode.id}>
        <div
          className={styles.node}
          data-node-type={graphNode.runtime?.type}
          data-equalizer={Boolean(graphNode.appearance.equalizer) || undefined}
          data-dynamics={Boolean(graphNode.appearance.dynamics) || undefined}
          style={{
            width:
              normalizeAudioGraphNodeSize(graphNode, graphNode.size)?.width ??
              (editorWidth ? Math.min(1600, Math.max(240, editorWidth)) : undefined),
            height: layout.height,
            minHeight: layout.minHeight,
          }}
        >
          <NodePortHandles node={graphNode} geometry={layout.ports} side="input" />
          <AudioNodeCard
            {...graphNode.appearance}
            title={audioGraphNodeTitle(graphNode)}
            optionsLabel={graphNode.appearance.optionsLabel ?? t('routing.nodes.options')}
            content={renderNodeContent?.(graphNode)}
            headerActions={controls?.power}
            muteControl={controls?.mute}
            poweredOff={graphNode.runtime?.control?.enabled === false}
            faceHeight={layout.faceHeight}
            faceMode={graphNodeFaceMode(graphNode)}
            onFaceHeightChange={layout.onFaceHeightChange}
            onContentHeightChange={layout.onContentHeightChange}
            inputCount={graphNode.inputPorts.length}
            onGainChange={
              onNodeGainChange ? (gainDb) => onNodeGainChange(graphNode.id, gainDb) : undefined
            }
            onChannelLayoutChange={
              settings.channelLayout
                ? (value) => settings.channelLayout?.(graphNode.id, value)
                : undefined
            }
            onEndpointChange={
              settings.endpoint ? (value) => settings.endpoint?.(graphNode.id, value) : undefined
            }
            onLayoutChange={() => updateNodeInternals(graphNode.id)}
            onParameterChange={
              onNodeParameterChange
                ? (parameterId, value) => onNodeParameterChange(graphNode.id, parameterId, value)
                : undefined
            }
            onParametersChange={
              onNodeParametersChange
                ? (updates) => onNodeParametersChange(graphNode.id, updates)
                : undefined
            }
            onProcessorEditorRetry={
              graphNode.appearance.hasProcessorEditor && processorEditors
                ? () => processorEditors.onRetry(graphNode.id)
                : undefined
            }
            outputCount={graphNode.outputPorts.length}
            processorEditor={processorEditorProps(
              graphNode.id,
              graphNode.appearance.hasProcessorEditor,
              processorEditors,
            )}
            selected={selected}
            showPorts={false}
          />
          <NodePortHandles node={graphNode} geometry={layout.ports} side="output" />
          <NodeResizeGrip
            minHeight={layout.minHeight}
            minWidth={graphNodeMinimumWidth(graphNode)}
          />
        </div>
      </NodeOptionsIdContext.Provider>
    </MidiNodeScope>
  );
}

function MidiNodeScope({ node, children }: { node: AudioGraphNode; children: ReactNode }) {
  return (
    <MidiNodeContext.Provider
      value={{
        nodeId: node.runtime?.binding?.nodeId ?? node.id,
        nodeName: audioGraphNodeTitle(node),
      }}
    >
      {children}
    </MidiNodeContext.Provider>
  );
}

function NodePortHandles({
  node,
  geometry,
  side,
}: {
  node: AudioGraphNode;
  geometry: Parameters<typeof audioNodePortTop>[0];
  side: AudioPortSide;
}) {
  const ports = side === 'input' ? node.inputPorts : node.outputPorts;
  return namedAudioPorts(ports).map(({ port, name, channel }, index) => (
    <AudioPortHandle
      channel={channel}
      name={name}
      top={audioNodePortTop(geometry, index, ports.length)}
      key={port.id}
      nodeId={node.id}
      port={port}
      side={side}
    />
  ));
}

function NodeResizeGrip({ minHeight, minWidth }: { minHeight: number; minWidth: number }) {
  return (
    <NodeResizeControl
      className={`${styles.resizeHandle} nodrag nopan`}
      // Keep the independent translate reset out of CSS transform lowering.
      style={{ translate: 'none' }}
      position="bottom-right"
      minWidth={minWidth}
      minHeight={minHeight}
      maxWidth={4096}
      maxHeight={4096}
      autoScale={false}
    >
      <svg aria-hidden="true" width="12" height="12" viewBox="0 0 12 12">
        <path d="m3 10 7-7M7 10l3-3" fill="none" stroke="currentColor" />
      </svg>
    </NodeResizeControl>
  );
}

function processorEditorProps(
  nodeId: string,
  available: boolean | undefined,
  context: NodeProcessorEditorContextValue | null,
) {
  if (!available || !context) return undefined;
  const status = context.statuses.get(nodeId) ?? {
    failure: null,
    height: null,
    nodeId,
    protocolVersion: 1 as const,
    resizable: false,
    state: 'closed' as const,
    width: null,
  };
  return {
    actionLabel: status.state === 'closed' ? context.labels.open : context.labels.retry,
    failureLabel: status.failure ? context.labels.failure[status.failure] : undefined,
    state: status.state,
  };
}

function AudioPortHandle({
  top,
  channel: channelNumber,
  name,
  nodeId,
  port,
  side,
}: {
  top: number;
  channel: number;
  name: string;
  nodeId: string;
  port: AudioGraphPort;
  side: 'input' | 'output';
}) {
  const isConnectionTarget = useConnection(
    (connection) =>
      connection.inProgress &&
      connection.toHandle?.nodeId === nodeId &&
      connection.toHandle.id === port.id,
  );

  return (
    <Tooltip
      kind="readout"
      nonInteractive
      label={`CH ${channelNumber}`}
      offset={7}
      opened={isConnectionTarget ? true : undefined}
      delay={isConnectionTarget ? 0 : 160}
      position={side === 'input' ? 'left' : 'right'}
    >
      {/* A named graphic: ports stay out of Tab order but expose node direction and channel. */}
      <Handle
        role="img"
        aria-label={`${name}, CH ${channelNumber}`}
        className={`${styles.handle} ${side === 'input' ? styles.inputHandle : ''}`}
        data-audio-node-id={nodeId}
        data-audio-port-id={port.id}
        data-audio-port-side={side}
        data-connection-target={isConnectionTarget || undefined}
        id={port.id}
        position={side === 'input' ? Position.Left : Position.Right}
        style={{
          top: `${top}px`,
        }}
        type={side === 'input' ? 'target' : 'source'}
      />
    </Tooltip>
  );
}
