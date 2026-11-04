import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { AudioGraphChangeKind } from '../../core/audio-graph/AudioGraphCanvas';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { ExternalProcessorDescriptor } from '../../core/domain/control/processors';
import type { FailedProcessorInstance } from '../../core/domain/control/status';
import { useUndoHistory } from '../../core/history/useUndoHistory';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import { GraphEditSession } from './GraphEditSession';
import type { GraphPresentation } from './graphPresentation';
import { useGraphApplication } from './useGraphApplication';
import { useProcessorParameterPublisher } from './useProcessorParameterPublisher';
import type { WorkspaceGraph } from './useWorkspaceGraph';

/** Wires canvas edits, Undo/Redo and parameter publishing to one GraphEditSession per owner. */
export function useGraphUpdates(
  ownership: { controlPort: ControlPort; graphId?: string },
  presentation: GraphPresentation,
  processors: readonly ExternalProcessorDescriptor[],
  graph: WorkspaceGraph,
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
