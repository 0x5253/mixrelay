import { useEffect, useState } from 'react';
import { timerScheduler } from '../../core/adapters/browser/timers';
import type { MixRelayStatus } from '../../core/domain/control/status';
import { structuralShare } from '../../core/domain/structuralShare';
import type { GraphPort } from '../../core/ports/control/GraphPort';
import type { TelemetryPort } from '../../core/ports/control/TelemetryPort';
import type { Scheduler } from '../../core/ports/Scheduler';
import { startAudioStatus } from './application/startAudioStatus';

type StatusPort = Pick<GraphPort, 'getStatus'> & Pick<TelemetryPort, 'subscribeState'>;

type Observation = { controlPort: StatusPort; value: MixRelayStatus } | null;

/** The first reply after a (re)start is always new, even when a port hands back the same
 * object; later replies keep the previous identity when nothing changed. */
function observation(controlPort: StatusPort, value: MixRelayStatus, first: boolean) {
  return (previous: Observation): Observation =>
    first
      ? { controlPort, value: { ...value } }
      : structuralShare(previous, { controlPort, value });
}

/** Completion-based polling prevents overlapping native requests and cancels on unmount. */
export function useAudioStatus(
  controlPort: StatusPort,
  refreshToken = 0,
  scheduler: Scheduler = timerScheduler,
) {
  // An unchanged reply keeps the previous object, so a quiet poll renders nothing. The
  // first reply after a (re)start is always a new object: an acknowledgement refresh
  // marks a new observation that confirmations compare against by identity.
  const [status, setStatus] = useState<Observation>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: Explicit acknowledgement refresh restarts polling without clearing the current observation.
  useEffect(() => {
    return startAudioStatus(controlPort, scheduler, (next, first) => {
      setStatus(next ? observation(controlPort, next, first) : null);
    });
  }, [controlPort, refreshToken, scheduler]);
  // Refreshing the same service retains its last observation until the reply.
  // A replacement service or a failed read must still hide the old health.
  return status?.controlPort === controlPort ? status.value : null;
}
