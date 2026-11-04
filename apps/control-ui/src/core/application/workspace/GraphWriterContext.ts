import type { RuntimeGraphDocument } from '../../domain/control/graph';
import { failure } from '../../domain/graph/WorkspaceEdits';
import type { GraphPort } from '../../ports/control/GraphPort';
import type { TelemetryPort } from '../../ports/control/TelemetryPort';
import type { WorkspacePort } from '../../ports/control/WorkspacePort';
import type { WorkspaceRuntime } from '../../ports/WorkspaceRuntime';
import type { WorkspaceView } from '../../ports/WorkspaceView';
import { GraphReadback } from './GraphReadback';
import { PendingParameters } from './PendingParameters';
import { waitForGraphRequest } from './waitForGraphRequest';

/** The engine capabilities the graph writer uses. */
export type GraphWriterPort = Pick<
  GraphPort & WorkspacePort & TelemetryPort,
  | 'applyGraph'
  | 'getGraph'
  | 'getGraphCatalog'
  | 'getGraphStatus'
  | 'retryProcessor'
  | 'updateNodeControl'
  | 'subscribeState'
  | 'isOperationPending'
>;

/**
 * The state one workspace writer shares between its collaborators: the running operation,
 * the queued draft and parameters, the acknowledged engine document and the recovery baseline.
 */
export class GraphWriterContext<Model extends object> {
  readonly readback: GraphReadback;
  readonly parameters = new PendingParameters();
  operation: AbortController | null = null;
  pending: Model | null = null;
  blocked = false;
  document: RuntimeGraphDocument | null = null;
  recoveryBaseline: RuntimeGraphDocument | null = null;

  constructor(
    readonly port: GraphWriterPort,
    readonly view: WorkspaceView<Model>,
    readonly runtime: WorkspaceRuntime<Model>,
    readonly timeoutMs: number,
  ) {
    this.readback = new GraphReadback(port, timeoutMs, failure, runtime.scheduler, runtime.clock);
  }

  /** No write runs and no failure waits for an explicit retry or reload. */
  get idle() {
    return !this.blocked && !this.operation;
  }

  /** The acknowledged engine document, or the view's before the first acknowledgement. */
  source() {
    return this.document ?? this.view.getDocument();
  }

  graphId() {
    return this.view.getGraphId?.();
  }

  reset(document: RuntimeGraphDocument | null) {
    this.operation?.abort();
    this.operation = null;
    this.pending = null;
    this.parameters.clear();
    this.blocked = false;
    this.document = document;
    this.recoveryBaseline = null;
  }

  start() {
    const operation = new AbortController();
    this.operation = operation;
    return operation;
  }

  announceApplying() {
    this.view.state('applying');
    this.view.error(null);
  }

  finish(operation: AbortController) {
    if (this.operation === operation) this.operation = null;
  }

  publish(document: RuntimeGraphDocument) {
    this.document = document;
    this.view.document(document);
  }

  guardOwner(signal: AbortSignal, graphId: string | undefined) {
    signal.throwIfAborted();
    if (this.graphId() !== graphId) {
      // Prevent recovery from reading/republishing the newly selected graph for an old writer.
      throw failure('graph_revision_stale');
    }
  }

  waitForRequest<T>(request: Promise<T>, signal: AbortSignal, operationId: string) {
    return waitForGraphRequest(
      request,
      { port: this.port, operationId, signal, timeoutMs: this.timeoutMs },
      this.view.permissionWaiting,
      failure,
      this.runtime.scheduler,
      this.runtime.clock,
    );
  }

  async readEngine(
    signal: AbortSignal,
    wait: boolean,
    expected?: { graphId: string; revision?: number },
    publication: 'immediate' | 'deferred' = 'immediate',
  ) {
    const graphId = expected?.graphId ?? this.graphId();
    const document = await this.readback.document(signal, graphId);
    if (wait) await this.readback.waitForRevision(document.revision, signal);
    this.guardOwner(signal, graphId);
    if (expected?.revision !== undefined && document.revision !== expected.revision)
      throw failure('graph_revision_stale');
    if (publication === 'immediate') {
      this.publish(document);
      // Publication can synchronously retire this writer (graph selection/unmount).
      this.guardOwner(signal, graphId);
    }
    return document;
  }

  /** Reconciles acknowledged controls into the view, or restores the engine document. */
  acceptControls(
    before: RuntimeGraphDocument,
    after: RuntimeGraphDocument,
    signal: AbortSignal,
    graphId: string | undefined,
    allowRestore = false,
  ) {
    if (this.view.reconcile) {
      const reconciled = this.view.reconcile(before, after);
      this.guardOwner(signal, graphId);
      if (!reconciled) {
        if (!allowRestore) throw failure('graph_revision_stale');
        this.view.restore(after);
      }
    } else this.view.restore(after);
    this.guardOwner(signal, graphId);
  }
}
