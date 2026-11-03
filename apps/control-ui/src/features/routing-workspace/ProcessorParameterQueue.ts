import { ControlPortError, type ProtocolErrorBody } from '../../core/domain/control/errors';
import type { ControlPort } from '../../core/ports/control/ControlPort';

export interface PendingProcessorParameter {
  expectedRevision: number;
  nodeId: string;
  parameterId: number;
  value: number;
}

export interface ProcessorParameterFailure {
  readonly requestId: number;
  readonly update: Readonly<PendingProcessorParameter>;
  readonly code: ProtocolErrorBody['code'];
}

interface Request {
  readonly id: number;
  readonly update: Readonly<PendingProcessorParameter>;
}

interface RejectedRequest extends ProcessorParameterFailure {
  readonly error: ControlPortError;
}

interface Callbacks {
  change: (busy: boolean, failures: readonly ProcessorParameterFailure[]) => void;
  persistenceWarning: (error: string | null) => void;
}

export type ParameterPort = Pick<ControlPort, 'setProcessorParameter'>;

function parameterKey(update: PendingProcessorParameter) {
  return JSON.stringify([update.nodeId, update.parameterId]);
}

function nativeError(error: unknown): ControlPortError {
  return error instanceof ControlPortError
    ? error
    : new ControlPortError(500, { code: 'control_request_failed', parameters: {} });
}

/** One port session owns its IPC sequence and latest intent per parameter. */
export class ProcessorParameterQueue {
  private readonly pending = new Map<string, Request>();
  private readonly rejected = new Map<string, RejectedRequest>();
  private active: Request | null = null;
  private task: Promise<void> | null = null;
  private nextId = 0;
  private closed = false;

  constructor(
    private readonly port: ParameterPort,
    private readonly callbacks: Callbacks,
  ) {}

  /** Queues user intent, coalescing only values not yet submitted to the engine. */
  publish(update: PendingProcessorParameter) {
    if (this.closed) return;
    this.pending.set(parameterKey(update), { id: ++this.nextId, update: { ...update } });
    this.start();
  }

  /** Explicit retry never replaces a newer queued or in-flight edit with an old value. */
  retryFailed() {
    if (this.closed) return;
    for (const [key, failure] of this.rejected) {
      const activeKey = this.active ? parameterKey(this.active.update) : null;
      if (this.pending.has(key) || activeKey === key) continue;
      this.pending.set(key, { id: ++this.nextId, update: failure.update });
    }
    this.start();
  }

  /** Call only after an explicit successful engine reload has replaced the draft. */
  discardFailures() {
    if (this.closed) return;
    this.rejected.clear();
    this.changed();
  }

  /** Waits for all submitted edits, rejecting when any parameter remains unapplied. */
  async whenIdle(): Promise<void> {
    while (this.task) await this.task;
    const failed = this.rejected.values().next().value;
    if (failed) throw failed.error;
  }

  /** Port replacement/unmount invalidates replies; it does not cancel native IPC already sent. */
  close() {
    this.closed = true;
    this.pending.clear();
    this.rejected.clear();
  }

  private changed() {
    if (!this.closed) this.callbacks.change(this.task !== null, [...this.rejected.values()]);
  }

  private start() {
    if (this.task || this.pending.size === 0 || this.closed) return;
    this.task = this.drain().finally(() => {
      this.task = null;
      this.active = null;
      this.changed();
      // A publish between drain completion and this finalizer still owns its queue entry.
      this.start();
    });
    this.changed();
  }

  private async drain() {
    while (!this.closed) {
      const entry = this.pending.entries().next().value;
      if (!entry) return;
      const [key, request] = entry;
      this.pending.delete(key);
      this.active = request;
      await this.send(key, request);
    }
  }

  private async send(key: string, request: Request) {
    const { nodeId, parameterId, value, expectedRevision } = request.update;
    try {
      const response = await this.port.setProcessorParameter(
        nodeId,
        parameterId,
        value,
        expectedRevision,
      );
      if (this.closed) return;
      const failure = this.rejected.get(key);
      if (failure && failure.requestId <= request.id) this.rejected.delete(key);
      this.callbacks.persistenceWarning(response.persistenceError ?? null);
    } catch (cause: unknown) {
      if (this.closed) return;
      const error = nativeError(cause);
      this.rejected.set(key, {
        requestId: request.id,
        update: request.update,
        code: error.body.code,
        error,
      });
      // Only this submitted generation failed. A newer pending value belongs
      // to a different request and must remain queued; unrelated errors remain visible.
    }
    this.changed();
  }
}
