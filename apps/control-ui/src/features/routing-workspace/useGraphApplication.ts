import { useEffect, useMemo, useRef } from 'react';
import { monotonicClock, timerScheduler } from '../../core/adapters/browser/timers';
import { GraphApplicationQueue } from '../../core/application/workspace/GraphApplicationQueue';
import { settleParametersForReload } from '../../core/application/workspace/settleParametersForReload';
import { type AudioGraphModel, normalizeAudioGraphNodeSize } from '../../core/audio-graph/model';
import type { GraphLayout } from '../../core/domain/control/catalog';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { ExternalProcessorDescriptor } from '../../core/domain/control/processors';
import type { FailedProcessorInstance } from '../../core/domain/control/status';
import { controlChanges } from '../../core/domain/graph/controlChanges';
import { rebaseGraphEdit } from '../../core/domain/graph/rebaseGraphEdit';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import type { WorkspaceView } from '../../core/ports/WorkspaceView';
import { rebaseControlsWithHistory } from './controlReconciliation';
import type { GraphPresentation } from './graphPresentation';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';
import type { useProcessorParameterPublisher } from './useProcessorParameterPublisher';

interface WorkspaceGraphView {
  graphId?: string;
  document: RuntimeGraphDocument | null;
  setDocument(document: RuntimeGraphDocument): void;
  setErrorCode(code: string | null): void;
  setPersistenceError(code: string | null): void;
  setState(state: 'active' | 'applying' | 'blocked'): void;
  setPermissionWaiting(waiting: boolean): void;
  setModel(model: AudioGraphModel): void;
}

interface WorkspacePresentation {
  presentation: GraphPresentation;
  processors: readonly ExternalProcessorDescriptor[];
  graph: WorkspaceGraphView;
  history: { clear(): void; rebase(transform: (model: AudioGraphModel) => AudioGraphModel): void };
  catalogRevision?: number;
  failedProcessors: FailedProcessorInstance[];
}

type ProcessorParameters = Pick<
  ReturnType<typeof useProcessorParameterPublisher>,
  'whenIdle' | 'discardFailures'
>;

function rebaseWorkspaceEdit(
  before: RuntimeGraphDocument,
  document: RuntimeGraphDocument,
  local: AudioGraphModel,
  presentation: GraphPresentation,
  processors: readonly ExternalProcessorDescriptor[],
) {
  const merged = rebaseGraphEdit(before, audioGraphToRuntimeGraph(local, before), document);
  const restored = runtimeGraphToAudioGraph(merged, presentation, processors);
  return {
    ...restored,
    nodes: restored.nodes.map((node) => {
      const previous = local.nodes.find((entry) => entry.id === node.id);
      return {
        ...node,
        position: previous?.position ?? node.position,
        size: previous?.size ?? node.size,
      };
    }),
  };
}

export function useGraphApplication(
  controlPort: ControlPort,
  presentation: GraphPresentation,
  processors: readonly ExternalProcessorDescriptor[],
  graph: WorkspaceGraphView,
  history: {
    clear: () => void;
    rebase: (transform: (model: AudioGraphModel) => AudioGraphModel) => void;
  },
  currentModel: { current: AudioGraphModel | null },
  parameters: ProcessorParameters,
  catalogRevision?: number,
  failedProcessors: FailedProcessorInstance[] = [],
) {
  const latest = useRef({
    presentation,
    processors,
    graph,
    history,
    catalogRevision,
    failedProcessors,
  });
  latest.current = { presentation, processors, graph, history, catalogRevision, failedProcessors };
  const { whenIdle, discardFailures } = parameters;
  const queue = useMemo(
    () =>
      new GraphApplicationQueue(
        controlPort,
        createWorkspaceView(latest, currentModel, { whenIdle, discardFailures }, controlPort),
        {
          scheduler: timerScheduler,
          clock: monotonicClock,
          operationId: () => crypto.randomUUID(),
          toDocument: audioGraphToRuntimeGraph,
        },
      ),
    [controlPort, currentModel, whenIdle, discardFailures],
  );
  useEffect(() => () => queue.reset(), [queue]);
  return queue;
}

function createWorkspaceView(
  latest: { current: WorkspacePresentation },
  currentModel: { current: AudioGraphModel | null },
  parameters: ProcessorParameters,
  controlPort: ControlPort,
): WorkspaceView<AudioGraphModel> {
  return {
    getGraphId: () => latest.current.graph.graphId,
    getCatalogRevision: () => latest.current.catalogRevision,
    hasFailedProcessor: (graphId, nodeId, instanceId) =>
      latest.current.failedProcessors.some(
        (failed) =>
          failed.graphId === graphId &&
          failed.nodeId === nodeId &&
          failed.instanceId === instanceId,
      ),
    beforeApply: parameters.whenIdle,
    beforeReload: () => settleParametersForReload(parameters.whenIdle),
    getReloadLayout: controlPort.getGraphLayout.bind(controlPort),
    getDocument: () => latest.current.graph.document,
    getModel: () => currentModel.current,
    document: (document) => latest.current.graph.setDocument(document),
    error: (code) => latest.current.graph.setErrorCode(code),
    persistenceWarning: (code) => latest.current.graph.setPersistenceError(code),
    state: (state) => latest.current.graph.setState(state),
    permissionWaiting: (waiting) => latest.current.graph.setPermissionWaiting(waiting),
    rebase: (before, document, local) => {
      const current = latest.current;
      const model = rebaseWorkspaceEdit(
        before,
        document,
        local,
        current.presentation,
        current.processors,
      );
      current.history.clear();
      currentModel.current = model;
      current.graph.setModel(model);
      return model;
    },
    reconcile: (before, document) => {
      const changes = controlChanges(before, document);
      const model = currentModel.current;
      if (!changes || !model) return false;
      const rebased = rebaseControlsWithHistory(model, changes, latest.current.history.rebase);
      const next = { ...rebased, revision: document.revision };
      currentModel.current = next;
      latest.current.graph.setModel(next);
      return true;
    },
    restore: (document, savedLayout) =>
      restoreWorkspaceModel(latest.current, currentModel, parameters, document, savedLayout),
  };
}

function restoreWorkspaceModel(
  current: WorkspacePresentation,
  currentModel: { current: AudioGraphModel | null },
  parameters: ProcessorParameters,
  document: RuntimeGraphDocument,
  savedLayout?: GraphLayout,
) {
  const restored = runtimeGraphToAudioGraph(document, current.presentation, current.processors);
  // Reload preserves displayed geometry and uses saved layout for new nodes.
  const model = {
    ...restored,
    nodes: restored.nodes.map((node) => {
      const old = currentModel.current?.nodes.find((entry) => entry.id === node.id);
      return {
        ...node,
        position: old?.position ?? savedLayout?.positions[node.id] ?? node.position,
        size: normalizeAudioGraphNodeSize(node, old ? old.size : savedLayout?.sizes?.[node.id]),
      };
    }),
  };
  currentModel.current = model;
  current.graph.setModel(model);
  current.history.clear();
  parameters.discardFailures();
}
