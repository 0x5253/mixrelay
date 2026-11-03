import { hasErrorCode } from '../../domain/control/errors';
import type {
  NodeControlRequest,
  NodeControlResponse,
  ProcessorParameterChange,
  ProcessorRetryRequest,
  RuntimeGraphDocument,
} from '../../domain/control/graph';
import { acknowledgedNodeControl } from '../../domain/graph/acknowledgedNodeControl';
import {
  definiteNodeRejection,
  errorCode,
  failedNodeControl,
  failure,
  hasSameProcessor,
  type NodeControlResult,
  requireRebasableConflict,
} from '../../domain/graph/WorkspaceEdits';
import type { GraphPort } from '../../ports/control/GraphPort';
import type { TelemetryPort } from '../../ports/control/TelemetryPort';
import type { WorkspacePort } from '../../ports/control/WorkspacePort';
import type { WorkspaceRuntime } from '../../ports/WorkspaceRuntime';
import type { WorkspaceView } from '../../ports/WorkspaceView';
import { GraphReadback, pauseGraphObservation } from './GraphReadback';
import { waitForGraphRequest } from './waitForGraphRequest';

/** The engine capabilities this module uses. */
type EngineControl = GraphPort & WorkspacePort & TelemetryPort;

export const GRAPH_APPLICATION_TIMEOUT_MS = 10_000;

/** How a definite revision rejection without a rebase contract was resolved. */
type ConflictRecovery = 'restored' | 'draft_kept' | 'unresolved';

/** A definitely rejected write and the contract that merges it onto the current document. */
interface RejectedEdit<Model> {
  before: RuntimeGraphDocument;
  model: Model;
  parameters: Map<string, ProcessorParameterChange>;
  rebase: NonNullable<WorkspaceView<Model>['rebase']>;
}

type ProcessorRetryTarget = Pick<
  ProcessorRetryRequest,
  'expectedCatalogRevision' | 'expectedInstanceId' | 'graphId' | 'nodeId'
>;

/** One writer per workspace; external changes refresh the acknowledged settings automatically. */
export class GraphApplicationQueue<Model extends object> {
  private readonly readback: GraphReadback;
  private operation: AbortController | null = null;
  private pending: Model | null = null;
  private pendingParameters = new Map<string, ProcessorParameterChange>();
  private blocked = false;
  private document: RuntimeGraphDocument | null = null;
  private recoveryBaseline: RuntimeGraphDocument | null = null;

  constructor(
    private readonly port: Pick<
      EngineControl,
      | 'applyGraph'
      | 'getGraph'
      | 'getGraphCatalog'
      | 'getGraphStatus'
      | 'retryProcessor'
      | 'updateNodeControl'
      | 'subscribeState'
      | 'isOperationPending'
    >,
    private readonly callbacks: WorkspaceView<Model>,
    private readonly runtime: WorkspaceRuntime<Model>,
  ) {
    this.readback = new GraphReadback(
      port,
      GRAPH_APPLICATION_TIMEOUT_MS,
      failure,
      runtime.scheduler,
      runtime.clock,
    );
  }

  get canPublishParameters() {
    return !this.blocked && !this.operation;
  }

  reset(document: RuntimeGraphDocument | null = null) {
    this.operation?.abort();
    this.operation = null;
    this.pending = null;
    this.pendingParameters.clear();
    this.blocked = false;
    this.document = document;
    this.recoveryBaseline = null;
  }

  submit(model: Model, changes: ProcessorParameterChange[] = []) {
    this.pending = model;
    for (const change of changes)
      this.pendingParameters.set(
        JSON.stringify([change.nodeId, change.typeId, change.parameterId]),
        change,
      );
    if (!this.blocked && !this.operation) void this.run('submit');
  }

  retry() {
    if (!this.operation) void this.run('retry');
  }

  reload() {
    if (!this.operation) void this.run('reload');
  }

  refresh() {
    if (this.canPublishParameters) void this.run('refresh');
  }

  /** Shares the graph writer and CAS baseline; never displays an unacknowledged power state. */
  async setNodeControl(
    graphId: string,
    nodeId: string,
    change: Omit<NodeControlRequest, 'expectedRevision'>,
    expectedRevision?: number,
  ): Promise<NodeControlResult> {
    const source = this.document ?? this.callbacks.getDocument();
    const rejection = this.nodeControlRejection(graphId, expectedRevision);
    if (rejection) return { state: 'rejected', code: rejection };
    let sent = false;
    let accepted = false;
    const operation = new AbortController();
    const { signal } = operation;
    this.operation = operation;
    this.callbacks.state('applying');
    this.callbacks.error(null);
    try {
      if (this.callbacks.beforeApply)
        await this.readback.bound(this.callbacks.beforeApply(), signal);
      signal.throwIfAborted();
      this.validateNodeControlSource(source, graphId, nodeId);
      sent = true;
      const clientOperationId = this.runtime.operationId();
      const response = await this.waitForRequest(
        this.port.updateNodeControl(graphId, nodeId, {
          clientOperationId,
          ...change,
          expectedRevision: source.revision,
        }),
        signal,
        clientOperationId,
      );
      accepted = true;
      await this.acceptNodeControl(source, { graphId, nodeId, change }, response, signal);
      this.callbacks.persistenceWarning?.(response.persistenceError ?? null);
      this.callbacks.state('active');
      return { state: 'applied', revision: response.revision };
    } catch (error) {
      if (!signal.aborted) await this.recoverNodeControl(error, signal, graphId, accepted);
      return failedNodeControl(error, sent, accepted);
    } finally {
      if (this.operation === operation) this.operation = null;
    }
  }

  /** Retries one failed processor through the graph writer without replacing its draft. */
  async retryFailedProcessor(target: ProcessorRetryTarget): Promise<NodeControlResult> {
    const source = this.document ?? this.callbacks.getDocument();
    if (!source) return { state: 'rejected', code: 'node_not_found' };
    const rejection = this.processorRetryRejection(source, target);
    if (rejection) return { state: 'rejected', code: rejection };

    const operation = new AbortController();
    const { signal } = operation;
    this.operation = operation;
    this.callbacks.state('applying');
    this.callbacks.error(null);
    const attempt = { accepted: false, sent: false };
    try {
      return await this.runProcessorRetry(source, target, signal, attempt);
    } catch (error) {
      return this.handleProcessorRetryError(error, signal, target.graphId, attempt);
    } finally {
      if (this.operation === operation) this.operation = null;
    }
  }

  private async runProcessorRetry(
    source: RuntimeGraphDocument,
    target: ProcessorRetryTarget,
    signal: AbortSignal,
    attempt: { accepted: boolean; sent: boolean },
  ): Promise<NodeControlResult> {
    if (this.callbacks.beforeApply) await this.readback.bound(this.callbacks.beforeApply(), signal);
    signal.throwIfAborted();
    this.validateProcessorRetrySource(source, target);
    attempt.sent = true;
    const request: ProcessorRetryRequest = {
      ...target,
      expectedRevision: source.revision,
    };
    const clientOperationId = this.runtime.operationId();
    const response = await this.waitForRequest(
      this.port.retryProcessor({ ...request, clientOperationId }),
      signal,
      clientOperationId,
    );
    attempt.accepted = true;
    if (response.revision !== source.revision + 1) throw failure('graph_revision_stale');
    if (response.state !== 'active') await this.readback.waitForRevision(response.revision, signal);
    signal.throwIfAborted();
    this.validateProcessorRetryAcknowledgement(source, target, response.revision);
    if (this.pending) throw failure('graph_draft_changed');

    // A retry updates only the acknowledged revision; the editor model stays authoritative.
    const acknowledged = { ...source, revision: response.revision };
    this.document = acknowledged;
    this.callbacks.document(acknowledged);
    this.callbacks.persistenceWarning?.(response.persistenceError ?? null);
    this.callbacks.state('active');
    return { state: 'applied', revision: response.revision };
  }

  private async handleProcessorRetryError(
    error: unknown,
    signal: AbortSignal,
    graphId: string,
    attempt: { accepted: boolean; sent: boolean },
  ): Promise<NodeControlResult> {
    if (!signal.aborted && this.callbacks.getGraphId?.() === graphId) {
      const code = errorCode(error);
      if (attempt.sent && (attempt.accepted || !definiteNodeRejection(error)))
        await this.recover(error, signal, graphId);
      else {
        this.callbacks.error(code);
        this.callbacks.state('active');
      }
    }
    return failedNodeControl(error, attempt.sent, attempt.accepted);
  }

  private processorRetryRejection(source: RuntimeGraphDocument, target: ProcessorRetryTarget) {
    if (!this.canPublishParameters) return 'graph_exchange_busy';
    if (this.callbacks.getGraphId?.() !== target.graphId) return 'graph_revision_stale';
    if (
      this.callbacks.getCatalogRevision &&
      this.callbacks.getCatalogRevision() !== target.expectedCatalogRevision
    )
      return 'graph_revision_stale';
    try {
      this.validateProcessorRetrySource(source, target);
    } catch (error) {
      return errorCode(error);
    }
    return null;
  }

  private validateProcessorRetrySource(source: RuntimeGraphDocument, target: ProcessorRetryTarget) {
    const node = source.nodes.find((candidate) => candidate.id === target.nodeId);
    if (node?.type !== 'provider') throw failure('node_not_found');
    if (node.control?.enabled === false) throw failure('node_disabled');
    const current = this.document ?? this.callbacks.getDocument();
    if (
      current?.revision !== source.revision ||
      this.callbacks.getGraphId?.() !== target.graphId ||
      (this.callbacks.getCatalogRevision !== undefined &&
        this.callbacks.getCatalogRevision() !== target.expectedCatalogRevision)
    )
      throw failure('graph_revision_stale');
    if (
      this.callbacks.hasFailedProcessor !== undefined &&
      !this.callbacks.hasFailedProcessor(target.graphId, target.nodeId, target.expectedInstanceId)
    )
      throw failure('processor_instance_stale');
  }

  private validateProcessorRetryAcknowledgement(
    source: RuntimeGraphDocument,
    target: ProcessorRetryTarget,
    responseRevision: number,
  ) {
    const currentDocuments = [this.document, this.callbacks.getDocument()].filter(
      (document): document is RuntimeGraphDocument => document !== null,
    );
    const acceptedRevision = source.revision + 1;
    if (
      this.callbacks.getGraphId?.() !== target.graphId ||
      responseRevision !== acceptedRevision ||
      currentDocuments.some(
        (document) =>
          document.revision !== source.revision && document.revision !== acceptedRevision,
      )
    )
      throw failure('graph_revision_stale');
    const currentCatalogRevision = this.callbacks.getCatalogRevision?.();
    if (
      currentCatalogRevision !== undefined &&
      currentCatalogRevision !== target.expectedCatalogRevision &&
      currentCatalogRevision !== target.expectedCatalogRevision + 1
    )
      throw failure('graph_revision_stale');
  }

  private async acceptNodeControl(
    source: RuntimeGraphDocument,
    command: {
      graphId: string;
      nodeId: string;
      change: Omit<NodeControlRequest, 'expectedRevision'>;
    },
    response: NodeControlResponse,
    signal: AbortSignal,
  ) {
    const { graphId, nodeId, change } = command;
    if (typeof response.requiresGraphReload !== 'boolean') throw failure('control_request_failed');
    if (response.revision !== source.revision && response.revision !== source.revision + 1)
      throw failure('graph_revision_stale');
    if (response.state !== 'active') await this.readback.waitForRevision(response.revision, signal);
    signal.throwIfAborted();
    this.validateNodeControlSource(source, graphId, nodeId);
    if (this.pending) throw failure('graph_draft_changed');
    const document = response.requiresGraphReload
      ? await this.readEngine(signal, true, { graphId, revision: response.revision })
      : acknowledgedNodeControl(source, nodeId, change, response.revision);
    if (this.pending) throw failure('graph_draft_changed');
    if (response.requiresGraphReload) this.callbacks.restore(document);
    else this.acceptControls(source, document, signal, graphId);
    this.guardOwner(signal, graphId);
    this.document = document;
    this.callbacks.document(document);
    this.recoveryBaseline = null;
  }

  private nodeControlRejection(graphId: string, expectedRevision?: number) {
    if (!this.canPublishParameters) return 'graph_exchange_busy';
    if (this.callbacks.getGraphId?.() !== graphId) return 'graph_revision_stale';
    const source = this.document ?? this.callbacks.getDocument();
    if (expectedRevision !== undefined && source?.revision !== expectedRevision)
      return 'graph_revision_stale';
    return null;
  }

  private acceptControls(
    before: RuntimeGraphDocument,
    after: RuntimeGraphDocument,
    signal: AbortSignal,
    graphId: string | undefined,
    allowRestore = false,
  ) {
    if (this.callbacks.reconcile) {
      const reconciled = this.callbacks.reconcile(before, after);
      this.guardOwner(signal, graphId);
      if (!reconciled) {
        if (!allowRestore) throw failure('graph_revision_stale');
        this.callbacks.restore(after);
      }
    } else this.callbacks.restore(after);
    this.guardOwner(signal, graphId);
  }

  private validateNodeControlSource(
    source: RuntimeGraphDocument | null,
    graphId: string,
    nodeId: string,
  ): asserts source is RuntimeGraphDocument {
    if (!source?.nodes.some((node) => node.id === nodeId)) throw failure('node_not_found');
    const current = this.document ?? this.callbacks.getDocument();
    if (current?.revision !== source.revision || this.callbacks.getGraphId?.() !== graphId)
      throw failure('graph_revision_stale');
  }

  private async recoverNodeControl(
    error: unknown,
    signal: AbortSignal,
    graphId: string,
    accepted: boolean,
  ) {
    if (this.callbacks.getGraphId?.() !== graphId) return;
    if (!accepted && !this.pending && definiteNodeRejection(error)) {
      try {
        const before = this.document ?? this.callbacks.getDocument();
        const document = await this.readEngine(signal, true, { graphId });
        if (!this.pending && before) {
          this.acceptControls(before, document, signal, graphId);
          this.callbacks.error(
            errorCode(error) === 'graph_revision_stale' ? null : errorCode(error),
          );
          this.callbacks.state('active');
          return;
        }
      } catch {
        if (signal.aborted) return;
      }
    }
    await this.recover(error, signal, graphId);
  }

  private waitForRequest<T>(request: Promise<T>, signal: AbortSignal, operationId: string) {
    return waitForGraphRequest(
      request,
      { port: this.port, operationId, signal, timeoutMs: GRAPH_APPLICATION_TIMEOUT_MS },
      this.callbacks.permissionWaiting,
      failure,
      this.runtime.scheduler,
      this.runtime.clock,
    );
  }

  private guardOwner(signal: AbortSignal, graphId: string | undefined) {
    signal.throwIfAborted();
    if (this.callbacks.getGraphId?.() !== graphId) {
      // Prevent recovery from reading/republishing the newly selected graph for an old writer.
      throw failure('graph_revision_stale');
    }
  }

  private async readEngine(
    signal: AbortSignal,
    wait: boolean,
    expected?: { graphId: string; revision?: number },
    publication: 'immediate' | 'deferred' = 'immediate',
  ) {
    const graphId = expected?.graphId ?? this.callbacks.getGraphId?.();
    const document = await this.readback.document(signal, graphId);
    if (wait) await this.readback.waitForRevision(document.revision, signal);
    this.guardOwner(signal, graphId);
    if (expected?.revision !== undefined && document.revision !== expected.revision)
      throw failure('graph_revision_stale');
    if (publication === 'immediate') {
      this.document = document;
      this.callbacks.document(document);
      // Publication can synchronously retire this writer (graph selection/unmount).
      this.guardOwner(signal, graphId);
    }
    return document;
  }

  private async apply(model: Model, changes: ProcessorParameterChange[], signal: AbortSignal) {
    // A previously accepted UI parameter edit must settle before native state is captured.
    if (this.callbacks.beforeApply) await this.readback.bound(this.callbacks.beforeApply(), signal);
    signal.throwIfAborted();
    const source = this.document ?? this.callbacks.getDocument();
    if (!source) throw failure('control_request_failed');
    const converted = this.runtime.toDocument(model, source);
    // Only advance the acknowledged engine baseline. Local undo/edit revisions must
    // not jump over a concurrent writer and accidentally bypass stale-revision checks.
    const document = { ...converted, revision: source.revision + 1 };
    const surviving = changes.filter(
      (change) => hasSameProcessor(source, change) && hasSameProcessor(document, change),
    );
    // Naming the graph keeps another client's selection change from redirecting this edit.
    const graphId = this.callbacks.getGraphId?.();
    const clientOperationId = this.runtime.operationId();
    const response = await this.waitForRequest(
      this.port.applyGraph({
        clientOperationId,
        ...document,
        ...(surviving.length ? { processorParameterChanges: surviving } : {}),
        ...(graphId === undefined ? {} : { expectedGraphId: graphId }),
      }),
      signal,
      clientOperationId,
    );
    if (response.revision !== document.revision) throw failure('graph_revision_stale');
    if (response.state !== 'active') await this.readback.waitForRevision(response.revision, signal);
    signal.throwIfAborted();
    // Persistence is a separate acknowledgement: audio has already applied. Do not
    // retry/rollback the graph or block subsequent edits because disk storage failed.
    this.callbacks.persistenceWarning?.(response.persistenceError ?? null);
    this.document = document;
    this.callbacks.document(document);
    this.recoveryBaseline = null;
  }

  /**
   * Adopts the engine document after a definite rejection that left no draft to rebase. An edit
   * submitted during the read was made on the rejected draft, so it is kept for an explicit retry.
   */
  private async restoreRevisionConflict(
    error: unknown,
    signal: AbortSignal,
    graphId?: string,
  ): Promise<ConflictRecovery> {
    if (!hasErrorCode(error, 'graph_revision_stale')) return 'unresolved';
    try {
      const document = await this.readEngine(signal, true, graphId ? { graphId } : undefined);
      if (this.pending) return 'draft_kept';
      this.pendingParameters.clear();
      this.callbacks.restore(document);
      this.guardOwner(signal, graphId);
      this.blocked = false;
      this.callbacks.error(null);
      this.callbacks.state('active');
      return 'restored';
    } catch {
      return 'unresolved';
    }
  }

  private async recover(error: unknown, signal: AbortSignal, graphId?: string) {
    if (signal.aborted || this.callbacks.getGraphId?.() !== graphId) return;
    // Taken before a conflict read publishes the engine document: a retry rebases from here.
    const baseline = this.document ?? this.callbacks.getDocument();
    const conflict = this.pending
      ? 'unresolved'
      : await this.restoreRevisionConflict(error, signal, graphId);
    if (conflict === 'restored' || signal.aborted) return;
    this.blocked = true;
    this.recoveryBaseline ??= baseline;
    // Refresh the engine baseline, never overwrite the user's draft/history on a failed edit.
    // A failed read leaves the queue blocked; retry/reload must read it again before writing.
    try {
      await this.readEngine(signal, false, graphId ? { graphId } : undefined);
    } catch {
      if (signal.aborted) return;
    }
    if (signal.aborted) return;
    if (graphId && this.callbacks.getGraphId?.() !== graphId) return;
    this.callbacks.error(conflict === 'draft_kept' ? 'graph_draft_changed' : errorCode(error));
    this.callbacks.state('blocked');
  }

  private async applyPending(model: Model, signal: AbortSignal) {
    const parameters = this.pendingParameters;
    const before = this.document ?? this.callbacks.getDocument();
    this.pendingParameters = new Map();
    this.pending = null;
    try {
      await this.apply(model, [...parameters.values()], signal);
    } catch (error) {
      if (!signal.aborted)
        this.pendingParameters = new Map([...parameters, ...this.pendingParameters]);
      const rebase = this.callbacks.rebase;
      if (hasErrorCode(error, 'graph_revision_stale') && before && rebase) {
        await this.rebaseRejectedEdit({ before, model, parameters, rebase }, signal);
        return;
      }
      throw error;
    }
  }

  private async rebaseRejectedEdit(rejected: RejectedEdit<Model>, signal: AbortSignal) {
    // An edit submitted during the rejected write supersedes it.
    const attempt = { draft: this.pending ?? rejected.model };
    this.pending = null;
    this.pendingParameters = new Map([...rejected.parameters, ...this.pendingParameters]);
    try {
      await this.rebaseUntilApplied(rejected, attempt, signal);
    } catch (error) {
      // Recovery keeps a pending draft instead of restoring the engine document over it.
      if (!signal.aborted) this.pending ??= attempt.draft;
      throw error;
    }
  }

  private async rebaseUntilApplied(
    rejected: RejectedEdit<Model>,
    attempt: { draft: Model },
    signal: AbortSignal,
  ) {
    const graphId = this.callbacks.getGraphId?.();
    const deadline = this.runtime.clock.now() + GRAPH_APPLICATION_TIMEOUT_MS;
    let baseline = rejected.before;
    while (true) {
      const current = await this.readEngine(signal, true, graphId ? { graphId } : undefined);
      // An edit submitted during the read, the backoff or the last write supersedes the draft.
      const rebased = rejected.rebase(baseline, current, this.pending ?? attempt.draft);
      this.guardOwner(signal, graphId);
      this.pending = null;
      attempt.draft = rebased;
      const parameters = this.pendingParameters;
      this.pendingParameters = new Map();
      try {
        await this.apply(rebased, [...parameters.values()], signal);
        return;
      } catch (error) {
        if (signal.aborted) throw error;
        this.pendingParameters = new Map([...parameters, ...this.pendingParameters]);
        requireRebasableConflict(error, deadline, this.runtime.clock.now());
        baseline = current;
        await pauseGraphObservation(signal, this.runtime.scheduler);
      }
    }
  }

  private acceptRefresh(
    before: RuntimeGraphDocument | null,
    document: RuntimeGraphDocument,
    signal: AbortSignal,
    graphId: string | undefined,
  ) {
    if (before && this.pending && this.callbacks.rebase) {
      const rebased = this.callbacks.rebase(before, document, this.pending);
      this.guardOwner(signal, graphId);
      this.pending = rebased;
    } else {
      if (this.pending) throw failure('graph_draft_changed');
      if (before) this.acceptControls(before, document, signal, graphId, true);
      else this.callbacks.restore(document);
    }
    this.guardOwner(signal, graphId);
    this.blocked = false;
    this.recoveryBaseline = null;
  }

  private async readReloadLayout(graphId: string | undefined, signal: AbortSignal) {
    if (!graphId || !this.callbacks.getReloadLayout) return undefined;
    const response = await this.readback.bound(this.callbacks.getReloadLayout(graphId), signal);
    signal.throwIfAborted();
    if (response.graphId !== graphId) throw failure('graph_layout_identity_mismatch');
    if (this.callbacks.getGraphId?.() !== graphId) throw failure('graph_revision_stale');
    return response.layout;
  }

  private async prepareRecovery(
    mode: 'retry' | 'reload' | 'refresh',
    signal: AbortSignal,
    initialModel: Model | null,
  ) {
    if (mode === 'reload' && this.callbacks.beforeReload)
      await this.readback.bound(this.callbacks.beforeReload(), signal);
    const before = this.recoveryBaseline ?? this.document ?? this.callbacks.getDocument();
    const graphId = this.callbacks.getGraphId?.();
    const document = await this.readEngine(
      signal,
      mode !== 'refresh',
      graphId ? { graphId } : undefined,
      mode === 'reload' ? 'deferred' : 'immediate',
    );
    if (mode === 'refresh') {
      this.acceptRefresh(before, document, signal, graphId);
      return;
    }
    if (mode === 'reload') {
      const savedLayout = await this.readReloadLayout(graphId, signal);
      if (this.callbacks.getModel() !== initialModel) throw failure('graph_draft_changed');
      this.pending = null;
      this.pendingParameters.clear();
      // Publishing the document refreshes presentation and therefore the model identity.
      // Defer that side effect until every asynchronous read and the draft guard have finished.
      this.document = document;
      this.callbacks.document(document);
      this.guardOwner(signal, graphId);
      this.callbacks.restore(document, savedLayout);
      this.guardOwner(signal, graphId);
      this.recoveryBaseline = null;
    } else {
      const local = this.callbacks.getModel();
      const rebased =
        before && local && this.callbacks.rebase
          ? this.callbacks.rebase(before, document, local)
          : local;
      this.guardOwner(signal, graphId);
      this.pending = rebased;
      this.recoveryBaseline = document;
    }
    this.blocked = false;
  }

  private async run(mode: 'submit' | 'retry' | 'reload' | 'refresh') {
    const operation = new AbortController();
    const { signal } = operation;
    this.operation = operation;
    const graphId = this.callbacks.getGraphId?.();
    this.callbacks.state('applying');
    this.callbacks.error(null);
    const initialModel = this.callbacks.getModel();
    try {
      if (mode !== 'submit') await this.prepareRecovery(mode, signal, initialModel);
      while (this.pending && !signal.aborted) {
        await this.applyPending(this.pending, signal);
      }
      if (!signal.aborted) this.callbacks.state('active');
    } catch (error) {
      if (!signal.aborted) await this.recover(error, signal, graphId);
    } finally {
      if (this.operation === operation) this.operation = null;
    }
  }
}
