import type { ProcessorRetryRequest, RuntimeGraphDocument } from '../../domain/control/graph';
import {
  definiteNodeRejection,
  errorCode,
  failedNodeControl,
  failure,
  type NodeControlResult,
} from '../../domain/graph/WorkspaceEdits';
import type { GraphRecovery } from './GraphRecovery';
import type { GraphWriterContext } from './GraphWriterContext';

export type ProcessorRetryTarget = Pick<
  ProcessorRetryRequest,
  'expectedCatalogRevision' | 'expectedInstanceId' | 'graphId' | 'nodeId'
>;

interface RetryAttempt {
  accepted: boolean;
  sent: boolean;
}

/** Retries one failed processor through the graph writer without replacing its draft. */
export class ProcessorRetryWriter<Model extends object> {
  constructor(
    private readonly context: GraphWriterContext<Model>,
    private readonly recovery: GraphRecovery<Model>,
  ) {}

  async retry(target: ProcessorRetryTarget): Promise<NodeControlResult> {
    const { context } = this;
    const source = context.source();
    if (!source) return { state: 'rejected', code: 'node_not_found' };
    const rejection = this.rejection(source, target);
    if (rejection) return { state: 'rejected', code: rejection };

    const operation = context.start();
    const { signal } = operation;
    context.announceApplying();
    const attempt: RetryAttempt = { accepted: false, sent: false };
    try {
      return await this.run(source, target, signal, attempt);
    } catch (error) {
      return this.fail(error, signal, target.graphId, attempt);
    } finally {
      context.finish(operation);
    }
  }

  private async run(
    source: RuntimeGraphDocument,
    target: ProcessorRetryTarget,
    signal: AbortSignal,
    attempt: RetryAttempt,
  ): Promise<NodeControlResult> {
    const { context } = this;
    if (context.view.beforeApply) await context.readback.bound(context.view.beforeApply(), signal);
    signal.throwIfAborted();
    this.validateSource(source, target);
    attempt.sent = true;
    const request: ProcessorRetryRequest = { ...target, expectedRevision: source.revision };
    const clientOperationId = context.runtime.operationId();
    const response = await context.waitForRequest(
      context.port.retryProcessor({ ...request, clientOperationId }),
      signal,
      clientOperationId,
    );
    attempt.accepted = true;
    if (response.revision !== source.revision + 1) throw failure('graph_revision_stale');
    if (response.state !== 'active')
      await context.readback.waitForRevision(response.revision, signal);
    signal.throwIfAborted();
    this.validateAcknowledgement(source, target, response.revision);
    if (context.pending) throw failure('graph_draft_changed');

    // A retry updates only the acknowledged revision; the editor model stays authoritative.
    context.publish({ ...source, revision: response.revision });
    context.view.persistenceWarning?.(response.persistenceError ?? null);
    context.view.state('active');
    return { state: 'applied', revision: response.revision };
  }

  private async fail(
    error: unknown,
    signal: AbortSignal,
    graphId: string,
    attempt: RetryAttempt,
  ): Promise<NodeControlResult> {
    const { context } = this;
    if (!signal.aborted && context.graphId() === graphId) {
      const code = errorCode(error);
      if (attempt.sent && (attempt.accepted || !definiteNodeRejection(error)))
        await this.recovery.recover(error, signal, graphId);
      else {
        context.view.error(code);
        context.view.state('active');
      }
    }
    return failedNodeControl(error, attempt.sent, attempt.accepted);
  }

  private rejection(source: RuntimeGraphDocument, target: ProcessorRetryTarget) {
    const { context } = this;
    if (!context.idle) return 'graph_exchange_busy';
    if (context.graphId() !== target.graphId) return 'graph_revision_stale';
    if (
      context.view.getCatalogRevision &&
      context.view.getCatalogRevision() !== target.expectedCatalogRevision
    )
      return 'graph_revision_stale';
    try {
      this.validateSource(source, target);
    } catch (error) {
      return errorCode(error);
    }
    return null;
  }

  private validateSource(source: RuntimeGraphDocument, target: ProcessorRetryTarget) {
    const { view } = this.context;
    const node = source.nodes.find((candidate) => candidate.id === target.nodeId);
    if (node?.type !== 'provider') throw failure('node_not_found');
    if (node.control?.enabled === false) throw failure('node_disabled');
    if (
      this.context.source()?.revision !== source.revision ||
      this.context.graphId() !== target.graphId ||
      (view.getCatalogRevision !== undefined &&
        view.getCatalogRevision() !== target.expectedCatalogRevision)
    )
      throw failure('graph_revision_stale');
    if (
      view.hasFailedProcessor !== undefined &&
      !view.hasFailedProcessor(target.graphId, target.nodeId, target.expectedInstanceId)
    )
      throw failure('processor_instance_stale');
  }

  private validateAcknowledgement(
    source: RuntimeGraphDocument,
    target: ProcessorRetryTarget,
    responseRevision: number,
  ) {
    const { context } = this;
    const currentDocuments = [context.document, context.view.getDocument()].filter(
      (document): document is RuntimeGraphDocument => document !== null,
    );
    const acceptedRevision = source.revision + 1;
    if (
      context.graphId() !== target.graphId ||
      responseRevision !== acceptedRevision ||
      currentDocuments.some(
        (document) =>
          document.revision !== source.revision && document.revision !== acceptedRevision,
      )
    )
      throw failure('graph_revision_stale');
    const currentCatalogRevision = context.view.getCatalogRevision?.();
    if (
      currentCatalogRevision !== undefined &&
      currentCatalogRevision !== target.expectedCatalogRevision &&
      currentCatalogRevision !== target.expectedCatalogRevision + 1
    )
      throw failure('graph_revision_stale');
  }
}
