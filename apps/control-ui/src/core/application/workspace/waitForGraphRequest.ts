import type { ControlPort } from '../../ports/control/ControlPort';
import type { Cancel, MonotonicClock, Scheduler } from '../../ports/Scheduler';

// First-use consent has a two-minute engine bound; allow ten seconds for its terminal reply.
const PERMISSION_LIMIT_MS = 130_000;
const OBSERVE_MS = 200;
type Port = Pick<ControlPort, 'getGraphStatus' | 'isOperationPending'>;

/** Extends an accepted operation only on fresh permission observations. */
export function waitForGraphRequest<T>(
  request: Promise<T>,
  context: { port: Port; operationId: string; signal: AbortSignal; timeoutMs: number },
  permissionWaiting: ((waiting: boolean) => void) | undefined,
  failure: (code: string) => Error,
  scheduler: Scheduler,
  clock: MonotonicClock,
): Promise<T> {
  const { port, operationId, signal, timeoutMs } = context;
  return new Promise((resolve, reject) => {
    let done = false;
    let waiting = false;
    let pollTimer: Cancel | undefined;
    const limit = clock.now() + PERMISSION_LIMIT_MS;
    let timer = scheduler.after(timeoutMs, () =>
      finish(() => reject(failure('graph_apply_timeout'))),
    );
    const finish = (settle: () => void) => {
      if (done) return;
      done = true;
      timer?.();
      pollTimer?.();
      signal.removeEventListener('abort', abort);
      if (waiting) permissionWaiting?.(false);
      settle();
    };
    const abort = () => finish(() => reject(signal.reason));
    const updateWaiting = (next: boolean) => {
      if (done) return;
      if (next !== waiting) permissionWaiting?.(next);
      if (next || waiting) {
        timer?.();
        timer = scheduler.after(Math.max(0, Math.min(timeoutMs, limit - clock.now())), () =>
          finish(() => reject(failure('graph_apply_timeout'))),
        );
      }
      waiting = next;
    };
    const observe = async () => {
      try {
        const status = await port.getGraphStatus();
        if (done) return;
        const reason = status.pendingReason === 'awaiting_microphone_permission';
        const accepted = reason && Boolean(await port.isOperationPending?.(operationId));
        updateWaiting(accepted);
      } catch {
        // Failed observations cannot renew the short deadline.
      }
      if (!done) pollTimer = scheduler.after(OBSERVE_MS, () => void observe());
    };
    signal.addEventListener('abort', abort, { once: true });
    if (signal.aborted) abort();
    request.then(
      (value) => finish(() => resolve(value)),
      (error) => finish(() => reject(error)),
    );
    if (!done && port.isOperationPending)
      pollTimer = scheduler.after(OBSERVE_MS, () => void observe());
  });
}
