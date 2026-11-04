import { useEffect, useRef } from 'react';
import type { GraphApplicationQueue } from '../../core/application/workspace/GraphApplicationQueue';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { ExternalProcessorDescriptor } from '../../core/domain/control/processors';
import type { GraphPresentation } from './graphPresentation';
import { runtimeGraphToAudioGraph } from './runtimeGraphProjection';
import type { WorkspaceGraph } from './useWorkspaceGraph';
import { preserveNodeGeometry, sameAuthoredGraph } from './workspaceGraphRefresh';

/** Adopts a session-supplied document once the workspace has nothing unapplied. */
export function useSessionGraphRefresh(
  document: RuntimeGraphDocument | undefined,
  graph: WorkspaceGraph,
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
    if (graph.document && sameAuthoredGraph(graph.document, document)) {
      setDocument(document);
      setModel((previous) => previous && { ...previous, revision: document.revision });
      setErrorCode(null);
      setState('active');
      return;
    }
    const next = runtimeGraphToAudioGraph(document, presentation, processors);
    setDocument(document);
    setModel((previous) => preserveNodeGeometry(next, previous));
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
