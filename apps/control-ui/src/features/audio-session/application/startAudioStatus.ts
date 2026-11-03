import type { MixRelayStatus } from '../../../core/domain/control/status';
import type { GraphPort } from '../../../core/ports/control/GraphPort';
import type { TelemetryPort } from '../../../core/ports/control/TelemetryPort';
import type { Cancel, Scheduler } from '../../../core/ports/Scheduler';

export type StatusPort = Pick<GraphPort, 'getStatus'> & Pick<TelemetryPort, 'subscribeState'>;

export function startAudioStatus(
  controlPort: StatusPort,
  scheduler: Scheduler,
  publish: (status: MixRelayStatus | null, first: boolean) => void,
) {
  let cancelled = false;
  let timer: Cancel | undefined;
  let first = true;
  let pushed = false;
  let generation = 0;
  const unsubscribe = controlPort.subscribeState?.((state) => {
    generation += 1;
    pushed = Boolean(state);
    if (state) {
      publish(state.status, first);
      first = false;
    } else publish(null, false);
  });
  const schedule = () => {
    if (!cancelled) timer = scheduler.after(1000, poll);
  };
  const poll = async () => {
    const attempt = generation;
    const current = () => !cancelled && attempt === generation;
    if (pushed) {
      schedule();
      return;
    }
    try {
      const next = await controlPort.getStatus();
      if (current()) publish(next, first);
      first = false;
    } catch {
      // Do not leave stale green health visible after the control service disappears.
      if (current()) publish(null, false);
    } finally {
      schedule();
    }
  };
  void poll();
  return () => {
    cancelled = true;
    unsubscribe?.();
    timer?.();
  };
}
