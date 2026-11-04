import type { TFunction } from 'i18next';
import {
  type ReactNode,
  type Ref,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';
import { timerScheduler as scheduler } from '../../core/adapters/browser/timers';
import type { GraphApplicationQueue } from '../../core/application/workspace/GraphApplicationQueue';
import {
  AudioGraphCanvas,
  type AudioGraphChangeKind,
  type ProcessorEditorLabels,
} from '../../core/audio-graph/AudioGraphCanvas';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import { audioGraphNodeTitle } from '../../core/audio-graph/model';
import type { PlaceGraphNodes } from '../../core/audio-graph/nodePlacement';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type {
  AudioDeviceDescriptor,
  CapturableApplication,
} from '../../core/domain/control/inventory';
import type { ExternalProcessorDescriptor } from '../../core/domain/control/processors';
import type {
  AudioSessionStatus,
  FailedProcessorInstance,
  VirtualEndpointSettings,
  VirtualEndpointSpec,
} from '../../core/domain/control/status';
import { useUndoHistory } from '../../core/history/useUndoHistory';
import { MidiContext, MidiGraphContext } from '../../core/midi/MidiContext';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import type { RecorderNodeInputs } from '../../core/ports/control/RecorderNodeView';
import type { Cancel } from '../../core/ports/Scheduler';
import type { EndpointControl } from '../../core/ui/AudioNodeEndpointOptions';
import { InlineNotice } from '../../core/ui/InlineNotice';
import { NodeMuteButton, NodePowerButton } from '../../core/ui/NodeSignalControls';
import { hasPlatformModifier, isNativeTextEntryTarget } from '../../core/ui/platformShortcut';
import { TitleField } from '../../core/ui/TitleField';
import { useSustainedProblem } from '../../core/ui/useSustainedProblem';
import { WorkspaceHeader } from '../../core/ui/WorkspaceHeader';
import { defaultEffectLabels } from './builtinEffects';
import { presentEqualizerSampleRate } from './equalizerPresentation';
import { GraphEditSession } from './GraphEditSession';
import { presentHardwareNodeStatus } from './hardwareNodeStatus';
import {
  addRoutingNode,
  audioGraphToRuntimeGraph,
  type GraphPresentation,
  presentRoutingApplicationIcons,
  runtimeGraphToAudioGraph,
  updateRecorderChannels,
  updateRoutingNodeGain,
  updateRoutingNodeParameter,
  updateRoutingNodeParameters,
  type VirtualEndpointUpdate,
} from './model';
import { routingNodeRequest } from './nodeRequests';
import { ProcessorEditorAlert } from './ProcessorControlAlerts';
import { ProcessorRecoveryControl } from './ProcessorRecoveryControl';
import { useProcessorScanReport } from './ProcessorScanReport';
import { processorCatalogPlaceholder } from './processorCatalogMenu';
import { presentProcessorFailures } from './processorFailures';
import { presentProcessorLayouts, useProcessorLayoutChange } from './processorLayouts';
import { presentProcessorTimings } from './processorTiming';
import styles from './RoutingWorkspace.module.css';
import {
  ADD_PROCESSOR_DIRECTORY_ITEM_ID,
  createNodeMenuItems,
  RESCAN_PROCESSORS_ITEM_ID,
} from './routingNodeMenus';
import { signalControlVisibility, usesRecoveryControls } from './signalControls';
import { useGraphApplication } from './useGraphApplication';
import { useGraphLayout } from './useGraphLayout';
import { useNodeControls } from './useNodeControls';
import { presentNodeDisplayNames, useNodeDisplayNameEditor } from './useNodeDisplayNameEditor';
import { useProcessorCatalog } from './useProcessorCatalog';
import { useProcessorEditors } from './useProcessorEditors';
import { useProcessorParameterPublisher } from './useProcessorParameterPublisher';
import { useRoutingInventory } from './useRoutingInventory';
import { useRoutingMeters } from './useRoutingMeters';
import { useVirtualDeviceAppActivity } from './useVirtualDeviceAppActivity';
import { useVirtualEndpointCreation } from './useVirtualEndpointCreation';
import { presentVirtualDeviceAppActivity } from './virtualDeviceAppActivity';
import { presentApplicationReselection, presentVirtualNodeStatus } from './virtualNodeStatus';
import {
  type WorkspaceProblemSummary,
  WorkspaceProblems,
  type WorkspaceState,
} from './WorkspaceProblems';

type UpdateModel = (model: AudioGraphModel, kind: AudioGraphChangeKind) => void;
const GRAPH_RECOVERY_MS = 500;

export interface RoutingWorkspaceProps {
  renderRecorder?: (input: RecorderNodeInputs) => ReactNode;
  processorCatalog?: ReturnType<typeof useProcessorCatalog>;
  deviceDriverAction?: {
    settings?: VirtualEndpointSettings;
    enabled: boolean;
    error: { endpointId: string | null; message: string } | null;
    change: (endpointId: string, update: VirtualEndpointUpdate) => void;
    create: (spec: VirtualEndpointSpec) => Promise<VirtualEndpointSpec | null>;
  };
  headerActions?: ReactNode;
  headerEnd?: ReactNode;
  notice?: ReactNode;
  /** The diagnostics window's problem list; workspace failures render only there. */
  problemsTarget?: HTMLElement | null;
  onProblemsChange?: (summary: WorkspaceProblemSummary) => void;
  failedProcessors?: FailedProcessorInstance[];
  catalogRevision?: number;
  processorTimings?: import('../../core/domain/control/status').ProcessorTiming[];
  graphId?: string;
  interactionDisabled?: boolean;
  sessionDocument?: RuntimeGraphDocument;
  audioSession?: AudioSessionStatus;
  playbackPaused?: boolean;
  controlPort: ControlPort;
  onBusyChange?: (busy: boolean) => void;
  onTitleChange?: (title: string) => void;
  title?: string;
}

function useGraphPresentation(): GraphPresentation {
  const { t } = useTranslation();
  return useMemo<GraphPresentation>(
    () => ({
      effectLabels: Object.fromEntries(
        Object.keys(defaultEffectLabels).map((key) => [key, t(`effects.${key}`)]),
      ),
      application: t('routing.nodes.application'),
      attack: t('routing.nodes.attack'),
      audioInput: t('routing.nodes.audioInput'),
      audioOutput: t('routing.nodes.audioOutput'),
      recorder: t('recorder.title'),
      channels: t('driverManagement.channels'),
      ceiling: t('routing.nodes.ceiling'),
      compressor: t('routing.nodes.compressor'),
      deviceName: t('routing.nodes.deviceName'),
      virtualSourcePort: t('routing.nodes.virtualSourcePort'),
      gain: t('routing.nodes.gain'),
      limiter: t('routing.nodes.limiter'),
      makeupGain: t('routing.nodes.makeupGain'),
      mixer: t('routing.nodes.mixer'),
      options: t('routing.nodes.options'),
      outputGain: t('routing.nodes.outputGain'),
      processor: t('routing.nodes.processor'),
      processorLayoutUnavailable: t('routing.nodes.processorLayoutUnavailable'),
      ports: {
        application: t('routing.ports.application'),
        deviceInput: t('routing.ports.deviceInput'),
        deviceOutput: t('routing.ports.deviceOutput'),
        effectInput: t('routing.ports.effectInput', { name: '{{name}}' }),
        effectOutput: t('routing.ports.effectOutput', { name: '{{name}}' }),
        gainInput: t('routing.ports.gainInput'),
        gainOutput: t('routing.ports.gainOutput'),
        mixerInput: t('routing.ports.mixerInput'),
        mixerOutput: t('routing.ports.mixerOutput'),
      },
      providerBusKinds: {
        main: t('routing.processorLayouts.main'),
        sidechain: t('routing.processorLayouts.sidechain'),
        auxiliary: t('routing.processorLayouts.auxiliary'),
      },
      ratio: t('routing.nodes.ratio'),
      release: t('routing.nodes.release'),
      routing: t('routing.groups.routing'),
      sourceGain: t('routing.nodes.sourceGain'),
      virtualSinkPort: t('routing.nodes.virtualSinkPort'),
      threshold: t('routing.nodes.threshold'),
      unsupported: t('routing.nodes.unsupported'),
      utility: t('routing.groups.utility'),
      virtualInput: t('routing.nodes.virtualInput'),
      virtualOutput: t('routing.nodes.virtualOutput'),
    }),
    [t],
  );
}

interface RoutingNodeCreationOptions {
  tryVirtualAdd: (
    itemId: string,
    position: { x: number; y: number },
    placeNodes?: PlaceGraphNodes,
  ) => boolean;
  audioSession?: AudioSessionStatus;
  applications: readonly CapturableApplication[];
  audioDevices: readonly AudioDeviceDescriptor[];
  model: AudioGraphModel | null;
  processors: readonly ExternalProcessorDescriptor[];
  presentation: GraphPresentation;
  refreshProcessors: (selectDirectory: boolean) => void;
  updateModel: UpdateModel;
}

function useRoutingNodeCreation(options: RoutingNodeCreationOptions) {
  const { t } = useTranslation();
  const {
    audioSession,
    applications,
    audioDevices,
    model,
    presentation,
    processors,
    refreshProcessors,
    updateModel,
    tryVirtualAdd,
  } = options;
  return useCallback(
    (itemId: string, position: { x: number; y: number }, placeNodes?: PlaceGraphNodes) => {
      if (tryVirtualAdd(itemId, position, placeNodes)) return;
      if (itemId === ADD_PROCESSOR_DIRECTORY_ITEM_ID) {
        refreshProcessors(true);
        return;
      }
      if (itemId === RESCAN_PROCESSORS_ITEM_ID) {
        refreshProcessors(false);
        return;
      }
      const request = routingNodeRequest(itemId, {
        applications,
        audioDevices,
        processors,
        systemAudioTitle:
          audioSession?.capabilities.backend === 'core_audio'
            ? t('studioGraph.nodes.systemAudio.name')
            : undefined,
      });
      if (!model || !request) return;
      const next = addRoutingNode(
        model,
        request,
        position,
        presentation,
        audioSession?.virtualEndpointSettings,
      );
      updateModel(placeNodes?.(next) ?? next, 'runtime');
    },
    [
      applications,
      audioDevices,
      audioSession,
      model,
      presentation,
      processors,
      refreshProcessors,
      updateModel,
      tryVirtualAdd,
      t,
    ],
  );
}

function useRoutingNodeGain(model: AudioGraphModel | null, updateModel: UpdateModel) {
  return useMemo(
    () =>
      model
        ? (nodeId: string, gainDb: number) =>
            updateModel(updateRoutingNodeGain(model, nodeId, gainDb), 'level')
        : undefined,
    [model, updateModel],
  );
}

function useRoutingNodeParameter(model: AudioGraphModel | null, updateModel: UpdateModel) {
  return useMemo(
    () =>
      model
        ? (nodeId: string, parameterId: string, value: number) => {
            updateModel(
              updateRoutingNodeParameter(model, nodeId, parameterId, value),
              parameterId.startsWith('external:')
                ? 'processor_parameter'
                : parameterId === 'preset'
                  ? 'runtime'
                  : 'level',
            );
          }
        : undefined,
    [model, updateModel],
  );
}

function useRoutingNodeParameters(model: AudioGraphModel | null, updateModel: UpdateModel) {
  return useMemo(
    () =>
      model
        ? (nodeId: string, updates: readonly { id: string; value: number }[]) =>
            updateModel(updateRoutingNodeParameters(model, nodeId, updates), 'parameter_group')
        : undefined,
    [model, updateModel],
  );
}

function virtualEndpointId(model: AudioGraphModel | null, nodeId: string) {
  const node = model?.nodes.find((candidate) => candidate.id === nodeId);
  const config = node?.runtime?.type === 'virtual_endpoint' ? node.runtime.config : null;
  return config && typeof config === 'object' && 'id' in config ? config.id : null;
}

// Endpoint settings are a Driver transaction; they are never authored into the graph.
function useVirtualEndpointUpdate(
  model: AudioGraphModel | null,
  action?: RoutingWorkspaceProps['deviceDriverAction'],
) {
  return useMemo(() => {
    if (!model || !action) return undefined;
    return (nodeId: string, update: VirtualEndpointUpdate) => {
      const endpointId = virtualEndpointId(model, nodeId);
      if (action.enabled && typeof endpointId === 'string') action.change(endpointId, update);
    };
  }, [model, action]);
}

function useWorkspaceGraph(
  controlPort: ControlPort,
  presentation: GraphPresentation,
  processors: readonly ExternalProcessorDescriptor[],
  sessionDocument?: RuntimeGraphDocument,
  graphId?: string,
) {
  const [document, setDocument] = useState<RuntimeGraphDocument | null>(null);
  const [model, setModel] = useState<AudioGraphModel | null>(null);
  const [state, setState] = useState<WorkspaceState>('loading');
  const [permissionWaiting, setPermissionWaiting] = useState(false);
  const [errorCode, setErrorCode] = useState<string | null>(null);
  const [persistenceError, setPersistenceError] = useState<string | null>(null);
  const conversion = useRef({ presentation, processors });
  conversion.current = { presentation, processors };
  useEffect(() => {
    // Catalog/localization refreshes update presentation, never fetch over a dirty draft.
    setModel((current) => {
      const source = document;
      if (!current || !source) return current;
      const refreshed = runtimeGraphToAudioGraph(
        audioGraphToRuntimeGraph(current, source),
        presentation,
        processors,
      );
      return {
        ...refreshed,
        nodes: refreshed.nodes.map((node) => {
          // A presentation-only serialization round trip preserves every visible node identity.
          const previous = current.nodes.find((old) => old.id === node.id) as AudioGraphNode;
          return { ...node, position: previous.position, size: previous.size };
        }),
      };
    });
  }, [presentation, processors, document]);
  useEffect(() => {
    if (sessionDocument) return;
    // A replacement port owns a different document. Do not let its faster layout
    // response restore onto the previous engine's model and consume the restore.
    setDocument(null);
    setModel(null);
    setState('loading');
    let cancelled = false;
    void controlPort
      .getGraph()
      .then((nextDocument) => {
        if (cancelled) return;
        setDocument(nextDocument);
        setModel(
          runtimeGraphToAudioGraph(
            nextDocument,
            conversion.current.presentation,
            conversion.current.processors,
          ),
        );
        setState('active');
      })
      .catch(() => {
        if (!cancelled) setState('unavailable');
      });
    return () => {
      cancelled = true;
    };
  }, [controlPort, sessionDocument]);
  return {
    graphId,
    document,
    errorCode,
    permissionWaiting,
    setPermissionWaiting,
    persistenceError,
    setPersistenceError,
    model,
    setDocument,
    setErrorCode,
    setModel,
    setState,
    state,
  };
}

function useUnavailableGraphRecovery(
  controlPort: ControlPort,
  presentation: GraphPresentation,
  processors: readonly ExternalProcessorDescriptor[],
  graph: ReturnType<typeof useWorkspaceGraph>,
) {
  const { setDocument, setErrorCode, setModel, setState, state } = graph;
  useEffect(() => {
    if (state !== 'unavailable') return;
    let cancelled = false;
    let retryTimer: Cancel;

    const recover = () => {
      void controlPort.getGraph().then(
        (nextDocument) => {
          if (cancelled) return;
          setDocument(nextDocument);
          setModel(runtimeGraphToAudioGraph(nextDocument, presentation, processors));
          setErrorCode(null);
          setState('active');
        },
        () => {
          if (!cancelled) retryTimer = scheduler.after(GRAPH_RECOVERY_MS, recover);
        },
      );
    };

    // A graph submission may already have succeeded when its follow-up status request encounters a
    // transient IPC failure. Reloading the authoritative graph keeps an empty graph valid and lets
    // the editor recover without requiring another user edit or an application restart.
    retryTimer = scheduler.after(GRAPH_RECOVERY_MS, recover);
    return () => {
      cancelled = true;
      retryTimer?.();
    };
  }, [controlPort, presentation, processors, setDocument, setErrorCode, setModel, setState, state]);
}

function runGraphHistoryShortcut(event: KeyboardEvent, undo: () => boolean, redo: () => boolean) {
  const key = event.key.toLowerCase();
  if (key === 'z') return event.shiftKey ? redo() : undo();
  return event.ctrlKey && key === 'y' ? redo() : false;
}

function useGraphHistoryShortcuts(undo: () => boolean, redo: () => boolean, enabled: boolean) {
  useEffect(() => {
    if (!enabled) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        event.altKey ||
        !hasPlatformModifier(event) ||
        isNativeTextEntryTarget(event.target)
      ) {
        return;
      }
      if (runGraphHistoryShortcut(event, undo, redo)) event.preventDefault();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [redo, undo, enabled]);
}

function useGraphUpdates(
  ownership: { controlPort: ControlPort; graphId?: string },
  presentation: GraphPresentation,
  processors: readonly ExternalProcessorDescriptor[],
  graph: ReturnType<typeof useWorkspaceGraph>,
  savePositions: (model: AudioGraphModel) => void,
  catalogRevision?: number,
  failedProcessors?: FailedProcessorInstance[],
) {
  const { controlPort, graphId } = ownership;
  const { model, setModel } = graph;
  const owner = useMemo(() => ({ controlPort, graphId }), [controlPort, graphId]);
  const currentModel = useRef(model);
  currentModel.current = model;
  const [session] = useState(() => new GraphEditSession(currentModel));
  const history = useUndoHistory<AudioGraphModel, AudioGraphChangeKind>();
  const { clear: clearHistory, rebase: rebaseHistory } = history;
  useEffect(() => {
    session.open(owner);
    clearHistory();
    return () => session.close(owner);
  }, [clearHistory, owner, session]);
  const parameters = useProcessorParameterPublisher(controlPort, graph.setPersistenceError);
  const applyModel = useGraphApplication(
    controlPort,
    presentation,
    processors,
    graph,
    { clear: clearHistory, rebase: rebaseHistory },
    currentModel,
    parameters,
    catalogRevision,
    failedProcessors,
  );
  session.render(owner, {
    history,
    queue: applyModel,
    publishParameter: parameters.publish,
    graphRevision: () => (graph.document as RuntimeGraphDocument).revision,
    show: setModel,
    savePositions,
  });
  // Canvas callbacks keep the owner they were created for.
  const updateModel = useCallback(
    (nextModel: AudioGraphModel, kind: AudioGraphChangeKind) =>
      session.edit(owner, nextModel, kind),
    [owner, session],
  );
  const undo = useCallback(() => session.undo(), [session]);
  const redo = useCallback(() => session.redo(), [session]);
  return {
    clearHistory,
    redo,
    undo,
    updateModel,
    application: applyModel,
    parameters,
    parameterBusy: parameters.busy || parameters.failures.length > 0,
  };
}

function useSessionGraphRefresh(
  document: RuntimeGraphDocument | undefined,
  graph: ReturnType<typeof useWorkspaceGraph>,
  presentation: GraphPresentation,
  processors: readonly ExternalProcessorDescriptor[],
  clearHistory: () => void,
  application: GraphApplicationQueue<AudioGraphModel>,
  parameterBusy: boolean,
) {
  const applied = useRef<RuntimeGraphDocument | undefined>(undefined);
  const { setDocument, setModel, setErrorCode, setState } = graph;
  useEffect(() => {
    if (!document || applied.current === document) return;
    // A native settings refresh is not permission to discard an unapplied graph.
    if (
      parameterBusy ||
      graph.state === 'blocked' ||
      graph.state === 'applying' ||
      (graph.model && document.revision < graph.model.revision)
    )
      return;
    applied.current = document;
    application.reset(document);
    // Playback advances the native revision without replacing the authored graph.
    // Keep the canvas and undo history while adopting the new command baseline.
    if (
      graph.document &&
      JSON.stringify(graph.document.nodes) === JSON.stringify(document.nodes) &&
      JSON.stringify(graph.document.edges) === JSON.stringify(document.edges)
    ) {
      setDocument(document);
      setModel((previous) => previous && { ...previous, revision: document.revision });
      setErrorCode(null);
      setState('active');
      return;
    }
    const next = runtimeGraphToAudioGraph(document, presentation, processors);
    setDocument(document);
    setModel((previous) => ({
      ...next,
      nodes: next.nodes.map((node) => ({
        ...node,
        position: previous?.nodes.find((old) => old.id === node.id)?.position ?? node.position,
        size: previous?.nodes.find((old) => old.id === node.id)?.size,
      })),
    }));
    clearHistory();
    setErrorCode(null);
    setState('active');
  }, [
    document,
    graph.state,
    graph.document,
    graph.model,
    parameterBusy,
    presentation,
    processors,
    clearHistory,
    application,
    setDocument,
    setModel,
    setErrorCode,
    setState,
  ]);
}

export function RoutingWorkspace({
  processorCatalog,
  failedProcessors,
  deviceDriverAction,
  graphId,
  interactionDisabled = false,
  sessionDocument,
  audioSession,
  controlPort,
  onBusyChange,
  ...viewOptions
}: RoutingWorkspaceProps) {
  if (deviceDriverAction && audioSession)
    audioSession = { ...audioSession, virtualEndpointSettings: deviceDriverAction.settings };
  const [menuOpened, setMenuOpened] = useState(false);
  const {
    inventory,
    applications,
    audioDevices,
    processors,
    refresh,
    processorCatalog: catalog,
  } = useWorkspaceInventory(controlPort, menuOpened, processorCatalog);
  const presentation = useGraphPresentation();
  const graph = useWorkspaceGraph(controlPort, presentation, processors, sessionDocument, graphId);
  useUnavailableGraphRecovery(controlPort, presentation, processors, graph);
  const { errorCode, model, state } = graph;
  const layout = useGraphLayout(controlPort, graphId, model, graph.setModel);
  const { clearHistory, redo, undo, updateModel, application, parameterBusy, parameters } =
    useGraphUpdates(
      { controlPort, graphId },
      presentation,
      processors,
      graph,
      layout.updateNodes,
      viewOptions.catalogRevision,
      failedProcessors,
    );
  useWorkspaceBusy(onBusyChange, state, parameterBusy, layout);
  useSessionGraphRefresh(
    sessionDocument,
    graph,
    presentation,
    processors,
    clearHistory,
    application,
    parameterBusy,
  );
  useGraphHistoryShortcuts(undo, redo, layout.loaded && !layout.loading && !interactionDisabled);
  const nodeActions = useWorkspaceNodeActions({
    audioDevices,
    audioSession,
    applications,
    controlPort,
    deviceDriverAction,
    interactionDisabled,
    model,
    presentation,
    processors,
    refreshProcessors: refresh,
    state,
    updateModel,
  });

  const viewProps = {
    ...viewOptions,
    ...nodeActions,
    failedProcessors,
    documentRevision: graph.document?.revision ?? 0,
    graphId,
    interactionDisabled,
    deviceDriverAction,
    processorCatalog: catalog,
    layout,
    application,
    audioSession,
    parameters,
    applications,
    audioDevices,
    controlPort,
    errorCode,
    inventory,
    onNodeMenuOpenedChange: setMenuOpened,
    persistenceError: graph.persistenceError,
    permissionWaiting: graph.permissionWaiting,
    model,
    processors,
    state: state === 'active' && parameters.failures.length > 0 ? ('blocked' as const) : state,
    updateModel,
  };
  return (
    <>
      <WorkspaceView {...viewProps} />
      {nodeActions.virtualCreation.dialog}
    </>
  );
}

function useWorkspaceNodeActions(options: {
  audioDevices: readonly AudioDeviceDescriptor[];
  audioSession?: AudioSessionStatus;
  applications: readonly CapturableApplication[];
  controlPort: ControlPort;
  deviceDriverAction?: RoutingWorkspaceProps['deviceDriverAction'];
  interactionDisabled: boolean;
  model: AudioGraphModel | null;
  presentation: GraphPresentation;
  processors: readonly ExternalProcessorDescriptor[];
  refreshProcessors: ReturnType<typeof useWorkspaceInventory>['refresh'];
  state: WorkspaceState;
  updateModel: UpdateModel;
}) {
  const virtualCreation = useVirtualEndpointCreation({
    owner: options.controlPort,
    model: options.model,
    settings: options.deviceDriverAction
      ? options.deviceDriverAction.settings
      : options.audioSession?.virtualEndpointSettings,
    createDevice: options.deviceDriverAction?.create,
    presentation: options.presentation,
    enabled:
      options.state === 'active' &&
      !options.interactionDisabled &&
      (options.deviceDriverAction
        ? options.deviceDriverAction.enabled
        : options.audioSession?.capabilities.virtualEndpoint === true),
    updateModel: (next) => options.updateModel(next, 'runtime'),
  });
  const addNode = useRoutingNodeCreation({
    tryVirtualAdd: virtualCreation.tryAdd,
    audioSession: options.audioSession,
    applications: options.applications,
    audioDevices: options.audioDevices,
    model: options.model,
    presentation: options.presentation,
    processors: options.processors,
    refreshProcessors: options.refreshProcessors,
    updateModel: options.updateModel,
  });
  const updateNodeGain = useRoutingNodeGain(options.model, options.updateModel);
  const updateNodeParameter = useRoutingNodeParameter(options.model, options.updateModel);
  const updateNodeParameters = useRoutingNodeParameters(options.model, options.updateModel);
  const updateChannelLayout = useProcessorLayoutChange(
    options.model,
    options.processors,
    options.presentation,
    options.updateModel,
  );
  const updateEndpoint = useVirtualEndpointUpdate(options.model, options.deviceDriverAction);
  return {
    addNode,
    updateChannelLayout,
    updateEndpoint,
    updateNodeGain,
    updateNodeParameter,
    updateNodeParameters,
    virtualCreation,
  };
}

function useWorkspaceBusy(
  onBusyChange: RoutingWorkspaceProps['onBusyChange'],
  state: WorkspaceState,
  parameterBusy: boolean,
  layout: ReturnType<typeof useGraphLayout>,
) {
  const busy = state !== 'active' || parameterBusy || layout.loading || layout.dirty;
  useEffect(() => onBusyChange?.(busy), [onBusyChange, busy]);
}

function useWorkspaceInventory(
  controlPort: ControlPort,
  menuOpened: boolean,
  suppliedCatalog: ReturnType<typeof useProcessorCatalog> | undefined,
) {
  const inventory = useRoutingInventory(controlPort, menuOpened);
  const processorCatalog = useProcessorCatalog(controlPort, suppliedCatalog);
  return {
    inventory,
    ...inventory,
    processorCatalog,
    processors: processorCatalog.processors,
    refresh: processorCatalog.refresh,
  };
}

interface WorkspaceViewProps {
  renderRecorder?: RoutingWorkspaceProps['renderRecorder'];
  permissionWaiting: boolean;
  documentRevision: number;
  headerActions?: ReactNode;
  headerEnd?: ReactNode;
  notice?: ReactNode;
  problemsTarget?: HTMLElement | null;
  onProblemsChange?: (summary: WorkspaceProblemSummary) => void;
  failedProcessors?: FailedProcessorInstance[];
  catalogRevision?: number;
  processorTimings?: RoutingWorkspaceProps['processorTimings'];
  graphId?: string;
  interactionDisabled: boolean;
  parameters: ReturnType<typeof useProcessorParameterPublisher>;
  deviceDriverAction?: RoutingWorkspaceProps['deviceDriverAction'];
  processorCatalog: ReturnType<typeof useProcessorCatalog>;
  layout: ReturnType<typeof useGraphLayout>;
  persistenceError: string | null;
  inventory: ReturnType<typeof useRoutingInventory>;
  onNodeMenuOpenedChange: (opened: boolean) => void;
  application: GraphApplicationQueue<AudioGraphModel>;
  audioSession?: AudioSessionStatus;
  playbackPaused?: boolean;
  addNode: (kind: string, position: { x: number; y: number }, placeNodes?: PlaceGraphNodes) => void;
  applications: readonly CapturableApplication[];
  audioDevices: readonly AudioDeviceDescriptor[];
  controlPort: ControlPort;
  errorCode: string | null;
  model: AudioGraphModel | null;
  processors: readonly ExternalProcessorDescriptor[];
  onTitleChange?: (title: string) => void;
  state: WorkspaceState;
  title?: string;
  updateModel: (model: AudioGraphModel, kind: AudioGraphChangeKind) => void;
  updateEndpoint?: (nodeId: string, update: VirtualEndpointUpdate) => void;
  updateChannelLayout?: (nodeId: string, layoutId: string) => void;
  updateNodeGain?: (nodeId: string, gainDb: number) => void;
  updateNodeParameter?: (nodeId: string, parameterId: string, value: number) => void;
  updateNodeParameters?: (
    nodeId: string,
    updates: readonly { id: string; value: number }[],
  ) => void;
}

function useProcessorEditorLabels() {
  const { t } = useTranslation();
  return useMemo<ProcessorEditorLabels>(
    () => ({
      open: t('routing.processorEditor.open'),
      retry: t('routing.processorControl.retry'),
      failure: {
        display_unavailable: t('routing.processorEditor.failures.displayUnavailable'),
        host_window_failed: t('routing.processorEditor.failures.hostWindowFailed'),
        no_editor: t('routing.processorEditor.failures.noEditor'),
        plugin_rejected: t('routing.processorEditor.failures.pluginRejected'),
        worker_unavailable: t('routing.processorEditor.failures.workerUnavailable'),
      },
    }),
    [t],
  );
}

function useDeviceActivityModel(props: WorkspaceViewProps) {
  const { t, i18n } = useTranslation();
  const settings = props.audioSession?.virtualEndpointSettings;
  const activity = useVirtualDeviceAppActivity(props.controlPort, settings);
  return useMemo(
    () =>
      props.model &&
      presentVirtualDeviceAppActivity(props.model, settings, activity, t, i18n.language),
    [props.model, settings, activity, t, i18n.language],
  );
}

function usePresentedModel(props: WorkspaceViewProps) {
  const { t } = useTranslation();
  const model = useDeviceActivityModel(props);
  const { applications, audioDevices, inventory, audioSession, deviceDriverAction } = props;
  // A graph apply usually settles well under a second; only a lasting one is a warning.
  const state = useSustainedProblem(props.state, (current) => current !== 'active');
  const presented = useMemo(
    () =>
      model
        ? presentVirtualNodeStatus(
            presentDriverEndpointControls(
              presentApplicationReselection(
                presentRoutingApplicationIcons(
                  presentHardwareNodeStatus(model, audioDevices, inventory.devicesObserved, t),
                  applications,
                ),
                t,
              ),
              deviceDriverAction,
              t,
            ),
            state,
            audioSession,
            t,
          )
        : null,
    [
      applications,
      audioDevices,
      inventory.devicesObserved,
      model,
      audioSession,
      state,
      t,
      deviceDriverAction,
    ],
  );
  return useMemo(
    () =>
      presented &&
      presentNodeDisplayNames(
        presentProcessorFailures(
          presentProcessorLayouts(
            presentEqualizerSampleRate(
              presentProcessorTimings(presented, props.processorTimings, props.graphId),
              audioSession?.format.sampleRate,
            ) as AudioGraphModel,
            props.processors,
            t,
          ),
          props.failedProcessors,
          props.graphId,
          t,
        ),
        props.layout.layout.nodeLabels,
      ),
    [
      presented,
      props.processors,
      props.failedProcessors,
      t,
      props.processorTimings,
      props.graphId,
      audioSession?.format.sampleRate,
      props.layout.layout.nodeLabels,
    ],
  );
}

function driverEndpointControl(
  control: EndpointControl,
  device: VirtualEndpointSettings['devices'][number] | undefined,
  action: NonNullable<RoutingWorkspaceProps['deviceDriverAction']>,
  endpointError: string | null,
  t: TFunction,
): EndpointControl {
  const editable = device && action.enabled;
  return {
    ...control,
    displayName: device?.spec.displayName ?? control.displayName,
    renderChannels: device?.spec.render?.channels ?? control.renderChannels,
    captureChannels: device?.spec.capture?.channels ?? control.captureChannels,
    canRename: Boolean(editable && action.settings?.canRename),
    allowedRenderChannels: editable ? action.settings?.renderChannels : [],
    allowedCaptureChannels: editable ? action.settings?.captureChannels : [],
    observedRate: device
      ? { label: t('audioSession.sampleRate'), value: device.spec.defaultSampleRate }
      : undefined,
    sampleRateOptions: editable ? device.spec.supportedSampleRates : undefined,
    rateError: endpointError,
    channelsLabel: t('driverManagement.channels'),
    nativeFormats: device?.nativeFormats,
    nativeFormatLabels: {
      bitDepth: t('driverManagement.bitDepth'),
      mixed: t('driverManagement.mixed'),
      unknown: t('driverManagement.unknown'),
    },
    formatsDisabled: !action.enabled,
    formatError: endpointError,
  };
}

function presentDriverEndpointControls(
  model: AudioGraphModel,
  action: RoutingWorkspaceProps['deviceDriverAction'],
  t: TFunction,
): AudioGraphModel {
  if (!action) return model;
  return {
    ...model,
    nodes: model.nodes.map((node) => {
      const control = node.appearance.endpointControl;
      if (!control) return node;
      // Endpoint controls are constructed only after the native endpoint spec is validated.
      const id = ((node.runtime as NonNullable<AudioGraphNode['runtime']>).config as { id: string })
        .id;
      const device = action.settings?.devices.find((candidate) => candidate.spec.id === id);
      const endpointError =
        action.error && action.error.endpointId === id ? action.error.message : null;
      return {
        ...node,
        appearance: {
          ...node.appearance,
          title: device?.spec.displayName ?? node.appearance.title,
          endpointControl: driverEndpointControl(control, device, action, endpointError, t),
        },
      };
    }),
  };
}

function routingContextLabels(t: TFunction) {
  return {
    renameNode: t('routing.nodeName.title'),
    connectChannels: t('routing.context.connectChannels'),
    removeNode: t('routing.context.removeNode'),
    removeSelection: t('routing.context.removeSelection'),
    disconnect: t('routing.context.disconnect'),
    disconnectPort: t('routing.context.disconnectPort'),
    disconnectPorts: t('routing.context.disconnectPorts'),
    removeCable: t('routing.context.removeCable'),
    fitView: t('routing.context.fitView'),
    selectAll: t('routing.context.selectAll'),
    clearSelection: t('routing.context.clearSelection'),
  };
}

function WorkspacePermissionNotice({ waiting }: { waiting: boolean }) {
  const { t } = useTranslation();
  return waiting ? (
    <InlineNotice tone="warning">{t('routing.states.awaitingMicrophonePermission')}</InlineNotice>
  ) : null;
}

function WorkspaceView(props: WorkspaceViewProps) {
  const { t } = useTranslation();
  const titleRef = useRef<HTMLInputElement>(null);
  const {
    addNode,
    controlPort,
    errorCode,
    model,
    state,
    updateModel,
    updateEndpoint,
    updateNodeGain,
    updateNodeParameter,
    updateNodeParameters,
  } = props;
  const meterStore = useRoutingMeters(controlPort);
  const nodeControls = useWorkspaceNodeControls(props);
  const editorModel = useMemo(
    () => (model ? { ...model, revision: props.documentRevision } : null),
    [model, props.documentRevision],
  );
  const processorEditors = useProcessorEditors(controlPort, editorModel, props.graphId);
  const scanReport = useProcessorScanReport(props.processorCatalog, addNode);
  const processorEditorLabels = useProcessorEditorLabels();
  const menuItems = useWorkspaceMenuItems(props, t);
  const presentedModel = usePresentedModel(props);
  const nodeRenderers = useNodeRenderers(props, t, nodeControls, presentedModel, processorEditors);
  const nodeNames = useNodeDisplayNameEditor(
    presentedModel,
    props.layout,
    props.interactionDisabled,
  );
  return (
    <WorkspaceShell props={props}>
      <WorkspaceToolbar
        titleRef={titleRef}
        title={props.title}
        onTitleChange={props.onTitleChange}
        headerActions={props.headerActions}
        headerEnd={props.headerEnd}
      />
      {props.notice}
      <WorkspacePermissionNotice waiting={props.permissionWaiting} />
      <WorkspaceProblems
        target={props.problemsTarget}
        onChange={props.onProblemsChange}
        errorCode={errorCode}
        state={state}
        application={props.application}
        parameters={props.parameters}
        inventory={props.inventory}
        persistenceError={props.persistenceError}
        layout={props.layout}
      />
      {presentedModel && props.layout.loaded ? (
        <div className={styles.canvas} inert={props.layout.loading}>
          <AudioGraphCanvas
            renderNodeControls={nodeRenderers.controls}
            renderNodeContent={nodeRenderers.content}
            viewport={props.layout.layout.viewport}
            onViewportChange={props.layout.updateViewport}
            contextMenuLabels={routingContextLabels(t)}
            addNodeLabel={t('routing.addNode')}
            ariaLabel={t('routing.canvasLabel')}
            model={presentedModel}
            meterStore={meterStore}
            nodeMenuItems={menuItems}
            onAddNode={scanReport.onMenuAction}
            onNodeMenuOpenedChange={props.onNodeMenuOpenedChange}
            onNodeRenameRequest={nodeNames.request}
            onModelChange={updateModel}
            onNodeEndpointChange={updateEndpoint}
            onNodeGainChange={updateNodeGain}
            onNodeParameterChange={updateNodeParameter}
            onNodeParametersChange={updateNodeParameters}
            onNodeChannelLayoutChange={props.updateChannelLayout}
            onProcessorEditorRetry={processorEditors.retry}
            processorEditorLabels={processorEditorLabels}
            processorEditorStatuses={processorEditors.statuses}
          />
        </div>
      ) : (
        <GraphPlaceholder
          state={state}
          layoutLoaded={props.layout.loaded}
          layoutFailed={Boolean(props.layout.error)}
        />
      )}
      {nodeNames.dialog}
      {scanReport.dialog}
    </WorkspaceShell>
  );
}

function WorkspaceShell({ props, children }: { props: WorkspaceViewProps; children: ReactNode }) {
  const { t } = useTranslation();
  return (
    <MidiGraphContext.Provider
      value={
        props.graphId
          ? { graphId: props.graphId, graphName: props.title ?? t('studio.untitledGraph') }
          : null
      }
    >
      <section className={styles.root} data-state={props.state}>
        {children}
      </section>
    </MidiGraphContext.Provider>
  );
}

function useWorkspaceMenuItems(props: WorkspaceViewProps, t: TFunction) {
  return createNodeMenuItems({
    applications: props.applications,
    audioDevices: props.audioDevices,
    processors: props.processors,
    model: props.model,
    t,
    session: props.audioSession,
    processorEmptyLabel: t(processorCatalogPlaceholder(props.processorCatalog)),
    processorReportDescription: props.processorCatalog.failure
      ? t('routing.processorControl.incomplete')
      : props.processorCatalog.diagnostics.length
        ? t('routing.processorControl.details', {
            count: new Set(props.processorCatalog.diagnostics.map((item) => JSON.stringify(item)))
              .size,
          })
        : undefined,
  });
}

/** A failed layout read says so here; its retry lives with the other problems in diagnostics. */
function GraphPlaceholder({
  state,
  layoutLoaded,
  layoutFailed,
}: {
  state: WorkspaceState;
  layoutLoaded: boolean;
  layoutFailed: boolean;
}) {
  const { t } = useTranslation();
  const placeholderState =
    state === 'unavailable' || layoutLoaded ? state : layoutFailed ? 'layoutFailed' : 'loading';
  return (
    <div
      key={placeholderState}
      className={styles.placeholder}
      role={placeholderState === 'unavailable' ? 'alert' : 'status'}
    >
      {t(`routing.states.${placeholderState}`)}
    </div>
  );
}

function WorkspaceToolbar({
  titleRef,
  title,
  onTitleChange,
  headerActions,
  headerEnd,
}: Pick<WorkspaceViewProps, 'title' | 'onTitleChange' | 'headerActions' | 'headerEnd'> & {
  titleRef: Ref<HTMLInputElement>;
}) {
  const { t } = useTranslation();
  const mapping = useContext(MidiContext)?.active ?? false;
  return (
    <WorkspaceHeader>
      <div className={styles.titleBlock}>
        <TitleField
          ref={titleRef}
          aria-label={t('studio.graphName')}
          readOnly={mapping}
          disabled={mapping}
          onBlur={(event) => {
            const normalized = event.currentTarget.value.trim();
            onTitleChange?.(normalized || t('studio.untitledGraph'));
          }}
          onChange={(event) => onTitleChange?.(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') event.currentTarget.blur();
          }}
          spellCheck={false}
          value={title ?? t('studio.untitledGraph')}
        />
      </div>
      {headerActions ? (
        <div
          className={styles.headerActions}
          inert={mapping}
          data-midi-inactive={mapping || undefined}
        >
          {headerActions}
        </div>
      ) : null}
      {headerEnd}
    </WorkspaceHeader>
  );
}

/** What node controls and recorder content read. Memoized so the canvas render functions
 * keep their identity and nodes re-render only when one of these changes. */
type NodeRenderInputs = Pick<
  WorkspaceViewProps,
  | 'application'
  | 'catalogRevision'
  | 'controlPort'
  | 'documentRevision'
  | 'failedProcessors'
  | 'graphId'
  | 'interactionDisabled'
  | 'playbackPaused'
  | 'renderRecorder'
  | 'state'
  | 'updateModel'
> & { idle: boolean; layoutDirty: boolean; mapping: boolean };

function useNodeRenderers(
  props: WorkspaceViewProps,
  t: TFunction,
  snapshot: import('../../core/domain/control/graph').GraphNodeControls | null,
  model: AudioGraphModel | null,
  editors: ReturnType<typeof useProcessorEditors>,
) {
  const idle = nodeControlIdle(props);
  const {
    application,
    catalogRevision,
    controlPort,
    documentRevision,
    failedProcessors,
    graphId,
    interactionDisabled,
    playbackPaused,
    renderRecorder,
    state,
    updateModel,
  } = props;
  const mapping = useContext(MidiContext)?.active ?? false;
  const layoutDirty = props.layout.dirty;
  const inputs = useMemo<NodeRenderInputs>(
    () => ({
      mapping,
      application,
      catalogRevision,
      controlPort,
      documentRevision,
      failedProcessors,
      graphId,
      idle,
      interactionDisabled,
      layoutDirty,
      playbackPaused,
      renderRecorder,
      state,
      updateModel,
    }),
    [
      mapping,
      application,
      catalogRevision,
      controlPort,
      documentRevision,
      failedProcessors,
      graphId,
      idle,
      interactionDisabled,
      layoutDirty,
      playbackPaused,
      renderRecorder,
      state,
      updateModel,
    ],
  );
  // Recorder edits apply to the committed model current when they are made, not the one
  // their node last rendered with; it is published on commit, never from a discarded render.
  const latestModel = useRef(model);
  useLayoutEffect(() => {
    latestModel.current = model;
  }, [model]);
  return {
    controls: useCallback(
      (node: AudioGraphNode) => nodeSignalControls(inputs, node, t, snapshot),
      [inputs, t, snapshot],
    ),
    content: useCallback(
      (node: AudioGraphNode) => {
        const issue = editors.issues.get(node.id);
        return issue ? (
          <ProcessorEditorAlert
            issue={{ ...issue, name: audioGraphNodeTitle(node) }}
            onRetry={() => editors.retry(node.id)}
          />
        ) : (
          recorderNodeContent(inputs, node, latestModel)
        );
      },
      [inputs, editors.issues, editors.retry],
    ),
  };
}

function recorderNodeContent(
  props: NodeRenderInputs,
  node: AudioGraphNode,
  model: { current: AudioGraphModel | null },
) {
  if (node.runtime?.type !== 'recorder' || !props.graphId) return null;
  return props.renderRecorder?.({
    controlPort: props.controlPort,
    graphId: props.graphId,
    nodeId: node.id,
    name: audioGraphNodeTitle(node),
    expectedRevision: props.documentRevision,
    poweredOn: node.runtime.control?.enabled !== false,
    playbackPaused: props.playbackPaused,
    enabled:
      props.state === 'active' &&
      !props.interactionDisabled &&
      node.runtime.control?.enabled !== false,
    channels: node.inputPorts.length,
    onChannelsChange: (channels) => {
      const current = model.current;
      if (current) props.updateModel(updateRecorderChannels(current, node.id, channels), 'runtime');
    },
  });
}

function nodeControlIdle(props: WorkspaceViewProps) {
  return (
    props.state === 'active' &&
    !props.interactionDisabled &&
    !props.parameters.busy &&
    props.parameters.failures.length === 0 &&
    !props.layout.loading &&
    !props.layout.saving
  );
}

function useWorkspaceNodeControls(props: WorkspaceViewProps) {
  const mapping = useContext(MidiContext)?.active ?? false;
  const snapshot = useNodeControls(
    props.controlPort,
    props.graphId,
    props.documentRevision,
    nodeControlIdle(props) || mapping,
    props.application,
  );
  return snapshot;
}

function nodeRecoveryControl(
  props: NodeRenderInputs,
  node: AudioGraphNode,
  enabled: boolean,
  disabled: boolean,
) {
  const failed =
    node.runtime?.type === 'provider'
      ? props.failedProcessors?.find(
          (candidate) => candidate.graphId === props.graphId && candidate.nodeId === node.id,
        )
      : undefined;
  const retryAvailable = failed !== undefined && props.catalogRevision !== undefined;
  return retryAvailable ? (
    <ProcessorRecoveryControl
      busy={disabled || props.layoutDirty}
      catalogRevision={props.catalogRevision}
      enabled={enabled}
      failedProcessors={props.failedProcessors}
      graphId={props.graphId}
      isProcessor={node.runtime?.type === 'provider'}
      nodeId={node.id}
      nodeName={audioGraphNodeTitle(node)}
      onRetry={(target) => props.application.retryFailedProcessor(target)}
      supported
    />
  ) : null;
}

function nodeSignalControls(
  props: NodeRenderInputs,
  node: AudioGraphNode,
  t: TFunction,
  snapshot: import('../../core/domain/control/graph').GraphNodeControls | null,
) {
  const control = node.runtime?.control ?? { enabled: true, muted: false };
  const nodeId = node.runtime?.binding?.nodeId ?? node.id;
  const entry = snapshot?.nodes.find((candidate) => candidate.nodeId === nodeId);
  const capabilities = signalControlVisibility(
    entry?.capabilities,
    control,
    usesRecoveryControls(node.runtime?.type, props.mapping),
  );
  if (!capabilities) return null;
  const disabled = !props.idle;
  const retry = nodeRecoveryControl(props, node, control.enabled, disabled);
  const retryAvailable = retry !== null;
  const graphId = props.graphId;
  if (!graphId || (!entry && !retryAvailable)) return null;
  // Native disabled buttons own interaction gating; the graph queue validates each live command.
  const change = (value: { enabled?: boolean; muted?: boolean }) => {
    void props.application.setNodeControl(graphId, nodeId, value);
  };
  return {
    power: (
      <>
        {retry}
        {capabilities.power ? (
          <NodePowerButton
            enabled={control.enabled}
            disabled={disabled}
            label={t(
              control.enabled ? 'routing.nodeControl.powerOff' : 'routing.nodeControl.powerOn',
              { name: audioGraphNodeTitle(node) },
            )}
            onChange={() => change({ enabled: !control.enabled })}
          />
        ) : null}
      </>
    ),
    mute: capabilities.mute ? (
      <NodeMuteButton
        muted={control.muted}
        disabled={disabled}
        label={t(control.muted ? 'routing.nodeControl.unmute' : 'routing.nodeControl.mute', {
          name: audioGraphNodeTitle(node),
        })}
        onChange={() => change({ muted: !control.muted })}
      />
    ) : null,
  };
}
