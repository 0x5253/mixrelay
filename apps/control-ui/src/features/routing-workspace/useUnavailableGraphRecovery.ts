import { useEffect } from 'react';
import { timerScheduler as scheduler } from '../../core/adapters/browser/timers';
import type { ExternalProcessorDescriptor } from '../../core/domain/control/processors';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import type { Cancel } from '../../core/ports/Scheduler';
import type { GraphPresentation } from './graphPresentation';
import { runtimeGraphToAudioGraph } from './runtimeGraphProjection';
import type { WorkspaceGraph } from './useWorkspaceGraph';

const GRAPH_RECOVERY_MS = 500;

/** Rereads the authoritative graph every half second while the engine is unavailable. */
export function useUnavailableGraphRecovery(
  controlPort: ControlPort,
  presentation: GraphPresentation,
  processors: readonly ExternalProcessorDescriptor[],
  graph: WorkspaceGraph,
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
