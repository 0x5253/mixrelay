import type {
  NodeControlRequest,
  NodeControlResponse,
  RuntimeGraphDocument,
} from '../../domain/control/graph';
import { acknowledgedNodeControl } from '../../domain/graph/acknowledgedNodeControl';
import {
  definiteNodeRejection,
  errorCode,
  failedNodeControl,
  failure,
  type NodeControlResult,
} from '../../domain/graph/WorkspaceEdits';
import type { GraphRecovery } from './GraphRecovery';
import type { GraphWriterContext } from './GraphWriterContext';

type NodeControlChange = Omit<NodeControlRequest, 'expectedRevision'>;

/** Node power and mute through the graph writer; never displays an unacknowledged state. */
export class NodeControlWriter<Model extends object> {
  constructor(
    private readonly context: GraphWriterContext<Model>,
    private readonly recovery: GraphRecovery<Model>,
  ) {}

  /** Shares the graph writer and CAS baseline with draft writes. */
  async set(
    graphId: string,
    nodeId: string,
    change: NodeControlChange,
    expectedRevision?: number,
  ): Promise<NodeControlResult> {
    const { context } = this;
    const source = context.source();
    const rejection = this.rejection(graphId, expectedRevision);
    if (rejection) return { state: 'rejected', code: rejection };
    let sent = false;
    let accepted = false;
    const operation = context.start();
    const { signal } = operation;
    context.announceApplying();
    try {
      if (context.view.beforeApply)
        await context.readback.bound(context.view.beforeApply(), signal);
      signal.throwIfAborted();
      this.validateSource(source, graphId, nodeId);
      sent = true;
      const clientOperationId = context.runtime.operationId();
      const response = await context.waitForRequest(
        context.port.updateNodeControl(graphId, nodeId, {
          clientOperationId,
          ...change,
          expectedRevision: source.revision,
        }),
        signal,
        clientOperationId,
      );
      accepted = true;
      await this.accept(source, { graphId, nodeId, change }, response, signal);
      context.view.persistenceWarning?.(response.persistenceError ?? null);
      context.view.state('active');
      return { state: 'applied', revision: response.revision };
    } catch (error) {
      if (!signal.aborted) await this.recover(error, signal, graphId, accepted);
      return failedNodeControl(error, sent, accepted);
    } finally {
      context.finish(operation);
    }
  }

  private rejection(graphId: string, expectedRevision?: number) {
    const { context } = this;
    if (!context.idle) return 'graph_exchange_busy';
    if (context.graphId() !== graphId) return 'graph_revision_stale';
    if (expectedRevision !== undefined && context.source()?.revision !== expectedRevision)
      return 'graph_revision_stale';
    return null;
  }

  private validateSource(
    source: RuntimeGraphDocument | null,
    graphId: string,
    nodeId: string,
  ): asserts source is RuntimeGraphDocument {
    if (!source?.nodes.some((node) => node.id === nodeId)) throw failure('node_not_found');
    if (this.context.source()?.revision !== source.revision || this.context.graphId() !== graphId)
      throw failure('graph_revision_stale');
  }

  private async accept(
    source: RuntimeGraphDocument,
    command: { graphId: string; nodeId: string; change: NodeControlChange },
    response: NodeControlResponse,
    signal: AbortSignal,
  ) {
    const { context } = this;
    const { graphId, nodeId, change } = command;
    if (typeof response.requiresGraphReload !== 'boolean') throw failure('control_request_failed');
    if (response.revision !== source.revision && response.revision !== source.revision + 1)
      throw failure('graph_revision_stale');
    if (response.state !== 'active')
      await context.readback.waitForRevision(response.revision, signal);
    signal.throwIfAborted();
    this.validateSource(source, graphId, nodeId);
    if (context.pending) throw failure('graph_draft_changed');
    const document = response.requiresGraphReload
      ? await context.readEngine(signal, true, { graphId, revision: response.revision })
      : acknowledgedNodeControl(source, nodeId, change, response.revision);
    if (context.pending) throw failure('graph_draft_changed');
    if (response.requiresGraphReload) context.view.restore(document);
    else context.acceptControls(source, document, signal, graphId);
    context.guardOwner(signal, graphId);
    context.publish(document);
    context.recoveryBaseline = null;
  }

  private async recover(error: unknown, signal: AbortSignal, graphId: string, accepted: boolean) {
    const { context } = this;
    if (context.graphId() !== graphId) return;
    if (!accepted && !context.pending && definiteNodeRejection(error)) {
      try {
        const before = context.source();
        const document = await context.readEngine(signal, true, { graphId });
        if (!context.pending && before) {
          context.acceptControls(before, document, signal, graphId);
          context.view.error(errorCode(error) === 'graph_revision_stale' ? null : errorCode(error));
          context.view.state('active');
          return;
        }
      } catch {
        if (signal.aborted) return;
      }
    }
    await this.recovery.recover(error, signal, graphId);
  }
}
