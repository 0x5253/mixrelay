import { waitForPushedRevision } from '../../application/graph/waitForPushedRevision';
import type { GraphPort } from '../../ports/control/GraphPort';
import type { TelemetryPort } from '../../ports/control/TelemetryPort';
import type { WorkspacePort } from '../../ports/control/WorkspacePort';
import type { MonotonicClock, Scheduler } from '../../ports/Scheduler';

/** The engine capabilities this module uses. */
type EngineControl = GraphPort & WorkspacePort & TelemetryPort;

const OBSERVATION_MS = 200;
type ReadPort = Pick<
  EngineControl,
  'getGraph' | 'getGraphCatalog' | 'getGraphStatus' | 'subscribeState'
>;

function boundedGraphRequest<T>(
  request: Promise<T>,
  signal: AbortSignal,
  timeoutMs: number,
  failure: (code: string) => Error,
  scheduler: Scheduler,
): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = scheduler.after(timeoutMs, () =>
      finish(() => reject(failure('graph_apply_timeout'))),
    );
    const cancel = () => finish(() => reject(signal.reason));
    const finish = (settle: () => void) => {
      timer?.();
      signal.removeEventListener('abort', cancel);
      settle();
    };
    signal.addEventListener('abort', cancel, { once: true });
    if (signal.aborted) cancel();
    request.then(
      (value) => finish(() => resolve(value)),
      (error) => finish(() => reject(error)),
    );
  });
}

export function pauseGraphObservation(signal: AbortSignal, scheduler: Scheduler): Promise<void> {
  if (signal.aborted) return Promise.reject(signal.reason);
  return new Promise((resolve, reject) => {
    const cancel = () => {
      timer?.();
      reject(signal.reason);
    };
    const timer = scheduler.after(OBSERVATION_MS, () => {
      signal.removeEventListener('abort', cancel);
      resolve();
    });
    signal.addEventListener('abort', cancel, { once: true });
  });
}

export class GraphReadback {
  constructor(
    private readonly port: ReadPort,
    private readonly timeoutMs: number,
    private readonly failure: (code: string) => Error,
    private readonly scheduler: Scheduler,
    private readonly clock: MonotonicClock,
  ) {}

  bound<T>(request: Promise<T>, signal: AbortSignal, timeoutMs = this.timeoutMs) {
    return boundedGraphRequest(request, signal, timeoutMs, this.failure, this.scheduler);
  }

  async waitForRevision(revision: number, signal: AbortSignal) {
    if (this.port.subscribeState)
      return waitForPushedRevision(
        this.port,
        revision,
        signal,
        this.timeoutMs,
        this.scheduler,
        this.failure,
      );
    const deadline = this.clock.now() + this.timeoutMs;
    while (true) {
      signal.throwIfAborted();
      const remaining = deadline - this.clock.now();
      if (remaining <= 0) throw this.failure('graph_apply_timeout');
      const status = await this.bound(this.port.getGraphStatus(), signal, remaining);
      signal.throwIfAborted();
      // A newer document is not confirmation of our edit: do not overwrite another writer.
      if (status.activeRevision > revision || status.submittedRevision > revision) {
        throw this.failure('graph_revision_stale');
      }
      if (status.activeRevision === revision) return;
      await pauseGraphObservation(signal, this.scheduler);
    }
  }

  /**
   * Reads this graph's document only while it is still selected. Another client can select a
   * different graph without changing the audio revision, and the engine publishes the new
   * document before its catalog, so separate reads cannot attribute `GET /graph` to a graph.
   * The catalog response names the selection and carries its document in one snapshot; a
   * lagging revision there only makes the next write stale, never attributes B's content to A.
   */
  async document(signal: AbortSignal, graphId: string | undefined) {
    if (graphId === undefined) return this.bound(this.port.getGraph(), signal);
    const catalog = await this.bound(this.port.getGraphCatalog(), signal);
    if (catalog.activeId !== graphId) throw this.failure('graph_catalog_revision_stale');
    return catalog.document;
  }
}
