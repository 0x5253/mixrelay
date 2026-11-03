import type { GraphPort } from '../../../core/ports/control/GraphPort';
import type { MonotonicClock, Scheduler } from '../../../core/ports/Scheduler';

type EnginePowerPort = Pick<GraphPort, 'getStatus'>;

/** Only native release/activation acknowledgements finish a switch, never local button intent. */
export async function waitForEnginePower(
  port: EnginePowerPort,
  enabled: boolean,
  current: () => boolean,
  scheduler: Scheduler,
  clock: MonotonicClock,
) {
  const deadline = clock.now() + 10000;
  while (current() && clock.now() < deadline) {
    const status = await port.getStatus();
    if (!current()) return null;
    if (status.enginePower?.enabled === enabled && !status.enginePower.transitioning) return status;
    await new Promise<void>((resolve) => scheduler.after(100, resolve));
  }
  if (!current()) return null;
  throw new Error('Engine power acknowledgement timed out');
}
