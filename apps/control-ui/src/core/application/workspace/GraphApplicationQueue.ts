import type {
  NodeControlRequest,
  ProcessorParameterChange,
  RuntimeGraphDocument,
} from '../../domain/control/graph';
import type { NodeControlResult } from '../../domain/graph/WorkspaceEdits';
import type { WorkspaceRuntime } from '../../ports/WorkspaceRuntime';
import type { WorkspaceView } from '../../ports/WorkspaceView';
import { GraphDraftWriter } from './GraphDraftWriter';
import { GraphRecovery, type RecoveryMode } from './GraphRecovery';
import { GraphWriterContext, type GraphWriterPort } from './GraphWriterContext';
import { NodeControlWriter } from './NodeControlWriter';
import { type ProcessorRetryTarget, ProcessorRetryWriter } from './ProcessorRetryWriter';

export const GRAPH_APPLICATION_TIMEOUT_MS = 10_000;

/** One writer per workspace; external changes refresh the acknowledged settings automatically. */
export class GraphApplicationQueue<Model extends object> {
  private readonly context: GraphWriterContext<Model>;
  private readonly drafts: GraphDraftWriter<Model>;
  private readonly recovery: GraphRecovery<Model>;
  private readonly nodeControls: NodeControlWriter<Model>;
  private readonly processorRetries: ProcessorRetryWriter<Model>;

  constructor(
    port: GraphWriterPort,
    callbacks: WorkspaceView<Model>,
    runtime: WorkspaceRuntime<Model>,
  ) {
    this.context = new GraphWriterContext(port, callbacks, runtime, GRAPH_APPLICATION_TIMEOUT_MS);
    this.drafts = new GraphDraftWriter(this.context);
    this.recovery = new GraphRecovery(this.context);
    this.nodeControls = new NodeControlWriter(this.context, this.recovery);
    this.processorRetries = new ProcessorRetryWriter(this.context, this.recovery);
  }

  get canPublishParameters() {
    return this.context.idle;
  }

  reset(document: RuntimeGraphDocument | null = null) {
    this.context.reset(document);
  }

  submit(model: Model, changes: ProcessorParameterChange[] = []) {
    this.context.pending = model;
    this.context.parameters.add(changes);
    if (this.context.idle) void this.run('submit');
  }

  retry() {
    if (!this.context.operation) void this.run('retry');
  }

  reload() {
    if (!this.context.operation) void this.run('reload');
  }

  refresh() {
    if (this.context.idle) void this.run('refresh');
  }

  /** Shares the graph writer and CAS baseline; never displays an unacknowledged power state. */
  setNodeControl(
    graphId: string,
    nodeId: string,
    change: Omit<NodeControlRequest, 'expectedRevision'>,
    expectedRevision?: number,
  ): Promise<NodeControlResult> {
    return this.nodeControls.set(graphId, nodeId, change, expectedRevision);
  }

  /** Retries one failed processor through the graph writer without replacing its draft. */
  retryFailedProcessor(target: ProcessorRetryTarget): Promise<NodeControlResult> {
    return this.processorRetries.retry(target);
  }

  private async run(mode: 'submit' | RecoveryMode) {
    const { context } = this;
    const operation = context.start();
    const { signal } = operation;
    const graphId = context.graphId();
    context.announceApplying();
    const initialModel = context.view.getModel();
    try {
      if (mode !== 'submit') await this.recovery.prepare(mode, signal, initialModel);
      while (context.pending && !signal.aborted) {
        await this.drafts.applyPending(context.pending, signal);
      }
      if (!signal.aborted) context.view.state('active');
    } catch (error) {
      if (!signal.aborted) await this.recovery.recover(error, signal, graphId);
    } finally {
      context.finish(operation);
    }
  }
}
