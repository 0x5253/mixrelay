import { useEffect, useRef, useState } from 'react';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { ExternalProcessorDescriptor } from '../../core/domain/control/processors';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import type { GraphPresentation } from './graphPresentation';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';
import type { WorkspaceState } from './WorkspaceProblems';
import { preserveNodeGeometry } from './workspaceGraphRefresh';

export type WorkspaceGraph = ReturnType<typeof useWorkspaceGraph>;

/** The engine document, its canvas model and the workspace state, loaded from `controlPort`
 * unless the session supplies the document. */
export function useWorkspaceGraph(
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
      // A presentation-only serialization round trip preserves every visible node identity.
      return preserveNodeGeometry(
        runtimeGraphToAudioGraph(
          audioGraphToRuntimeGraph(current, source),
          presentation,
          processors,
        ),
        current,
      );
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
