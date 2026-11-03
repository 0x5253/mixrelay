import { useEffect, useRef } from 'react';
import { timerScheduler } from '../../core/adapters/browser/timers';
import type { GraphApplicationQueue } from '../../core/application/workspace/GraphApplicationQueue';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { GraphNodeControls } from '../../core/domain/control/graph';
import type { GraphPort } from '../../core/ports/control/GraphPort';
import type { TelemetryPort } from '../../core/ports/control/TelemetryPort';
import type { Scheduler } from '../../core/ports/Scheduler';
import { useSharedState } from '../../core/ui/useStructuralShare';
import { startNodeControls } from './application/startNodeControls';

type NodeControlsPort = Pick<GraphPort, 'getNodeControls'> & Pick<TelemetryPort, 'subscribeState'>;

/** A single bounded-rate reader; stale graph/port replies cannot affect a replacement workspace. */
export function useNodeControls(
  port: NodeControlsPort,
  graphId: string | undefined,
  revision: number,
  idle: boolean,
  application: GraphApplicationQueue<AudioGraphModel>,
  scheduler: Scheduler = timerScheduler,
) {
  const [snapshot, setSnapshot] = useSharedState<GraphNodeControls | null>(null);
  const latest = useRef({ revision, idle });
  latest.current = { revision, idle };
  useEffect(() => {
    setSnapshot(null);
    if (!graphId) return;
    return startNodeControls(port, graphId, scheduler, {
      current: () => ({
        revision: latest.current.revision,
        idle: latest.current.idle && application.canPublishParameters,
      }),
      refresh: () => application.refresh(),
      publish: setSnapshot,
    });
  }, [port, graphId, application, setSnapshot, scheduler]);
  return snapshot;
}
