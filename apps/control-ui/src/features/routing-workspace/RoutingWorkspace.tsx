import { type ReactNode, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AudioGraphCanvas } from '../../core/audio-graph/AudioGraphCanvas';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type {
  AudioSessionStatus,
  FailedProcessorInstance,
  ProcessorTiming,
} from '../../core/domain/control/status';
import { MidiGraphContext } from '../../core/midi/MidiContext';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import type { RecorderNodeInputs } from '../../core/ports/control/RecorderNodeView';
import { InlineNotice } from '../../core/ui/InlineNotice';
import type { DeviceDriverAction } from './driverEndpointControls';
import { GraphPlaceholder } from './GraphPlaceholder';
import { useNodeRenderers, useWorkspaceNodeControls } from './nodeRenderers';
import { useProcessorScanReport } from './ProcessorScanReport';
import styles from './RoutingWorkspace.module.css';
import { useGraphHistoryShortcuts } from './useGraphHistoryShortcuts';
import { useGraphLayout } from './useGraphLayout';
import { useGraphPresentation } from './useGraphPresentation';
import { useGraphUpdates } from './useGraphUpdates';
import { useNodeDisplayNameEditor } from './useNodeDisplayNameEditor';
import { usePresentedModel } from './usePresentedModel';
import { useProcessorCatalog } from './useProcessorCatalog';
import { useProcessorEditors } from './useProcessorEditors';
import { useRoutingInventory } from './useRoutingInventory';
import { useRoutingMeters } from './useRoutingMeters';
import { useSessionGraphRefresh } from './useSessionGraphRefresh';
import { useUnavailableGraphRecovery } from './useUnavailableGraphRecovery';
import { useWorkspaceGraph } from './useWorkspaceGraph';
import { useWorkspaceNodeActions } from './useWorkspaceNodeActions';
import {
  type WorkspaceProblemSummary,
  WorkspaceProblems,
  type WorkspaceState,
} from './WorkspaceProblems';
import { WorkspaceToolbar } from './WorkspaceToolbar';
import {
  routingContextLabels,
  useProcessorEditorLabels,
  workspaceMenuItems,
} from './workspaceLabels';
import type { WorkspaceViewProps } from './workspaceViewProps';

export interface RoutingWorkspaceProps {
  renderRecorder?: (input: RecorderNodeInputs) => ReactNode;
  processorCatalog?: ReturnType<typeof useProcessorCatalog>;
  deviceDriverAction?: DeviceDriverAction;
  headerActions?: ReactNode;
  headerEnd?: ReactNode;
  notice?: ReactNode;
  /** The diagnostics window's problem list; workspace failures render only there. */
  problemsTarget?: HTMLElement | null;
  onProblemsChange?: (summary: WorkspaceProblemSummary) => void;
  failedProcessors?: FailedProcessorInstance[];
  catalogRevision?: number;
  processorTimings?: ProcessorTiming[];
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
  const menuItems = workspaceMenuItems(props, t);
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
