import type { GraphPort } from '../../../core/ports/control/GraphPort';
import type { MonotonicClock, Scheduler } from '../../../core/ports/Scheduler';

type ActivationPort = Pick<GraphPort, 'getGraphStatus'>;

/** Wait for native adoption after a format write. Cancellation stops further reads;
 * deadline or control failures propagate so the caller retains its existing document. */
export async function waitForGraphActivation(
  controlPort: ActivationPort,
  revision: number,
  isCurrent: () => boolean,
  scheduler: Scheduler,
  clock: MonotonicClock,
) {
  const deadline = clock.now() + 3000;
  while (clock.now() < deadline) {
    if (!isCurrent()) return false;
    const status = await controlPort.getGraphStatus();
    if (!isCurrent()) return false;
    if (status.activeRevision >= revision) return true;
    await new Promise<void>((resolve) => scheduler.after(25, resolve));
  }
  if (!isCurrent()) return false;
  throw new Error('Graph activation timed out');
}
