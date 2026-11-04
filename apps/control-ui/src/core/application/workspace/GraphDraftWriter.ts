import { hasErrorCode } from '../../domain/control/errors';
import type { ProcessorParameterChange, RuntimeGraphDocument } from '../../domain/control/graph';
import {
  failure,
  hasSameProcessor,
  requireRebasableConflict,
} from '../../domain/graph/WorkspaceEdits';
import type { WorkspaceView } from '../../ports/WorkspaceView';
import { pauseGraphObservation } from './GraphReadback';
import type { GraphWriterContext } from './GraphWriterContext';

/** A definitely rejected write and the contract that merges it onto the current document. */
interface RejectedEdit<Model> {
  before: RuntimeGraphDocument;
  model: Model;
  parameters: ReadonlyMap<string, ProcessorParameterChange>;
  rebase: NonNullable<WorkspaceView<Model>['rebase']>;
}

/** Writes the queued draft and its parameters, rebasing a stale write onto the engine. */
export class GraphDraftWriter<Model extends object> {
  constructor(private readonly context: GraphWriterContext<Model>) {}

  async applyPending(model: Model, signal: AbortSignal) {
    const { context } = this;
    const parameters = context.parameters.take();
    const before = context.source();
    context.pending = null;
    try {
      await this.apply(model, [...parameters.values()], signal);
    } catch (error) {
      if (!signal.aborted) context.parameters.restore(parameters);
      const rebase = context.view.rebase;
      if (hasErrorCode(error, 'graph_revision_stale') && before && rebase) {
        await this.rebaseRejectedEdit({ before, model, parameters, rebase }, signal);
        return;
      }
      throw error;
    }
  }

  private async apply(model: Model, changes: ProcessorParameterChange[], signal: AbortSignal) {
    const { context } = this;
    // A previously accepted UI parameter edit must settle before native state is captured.
    if (context.view.beforeApply) await context.readback.bound(context.view.beforeApply(), signal);
    signal.throwIfAborted();
    const source = context.source();
    if (!source) throw failure('control_request_failed');
    const converted = context.runtime.toDocument(model, source);
    // Only advance the acknowledged engine baseline. Local undo/edit revisions must
    // not jump over a concurrent writer and accidentally bypass stale-revision checks.
    const document = { ...converted, revision: source.revision + 1 };
    const surviving = changes.filter(
      (change) => hasSameProcessor(source, change) && hasSameProcessor(document, change),
    );
    // Naming the graph keeps another client's selection change from redirecting this edit.
    const graphId = context.graphId();
    const clientOperationId = context.runtime.operationId();
    const response = await context.waitForRequest(
      context.port.applyGraph({
        clientOperationId,
        ...document,
        ...(surviving.length ? { processorParameterChanges: surviving } : {}),
        ...(graphId === undefined ? {} : { expectedGraphId: graphId }),
      }),
      signal,
      clientOperationId,
    );
    if (response.revision !== document.revision) throw failure('graph_revision_stale');
    if (response.state !== 'active')
      await context.readback.waitForRevision(response.revision, signal);
    signal.throwIfAborted();
    // Persistence is a separate acknowledgement: audio has already applied. Do not
    // retry/rollback the graph or block subsequent edits because disk storage failed.
    context.view.persistenceWarning?.(response.persistenceError ?? null);
    context.publish(document);
    context.recoveryBaseline = null;
  }

  private async rebaseRejectedEdit(rejected: RejectedEdit<Model>, signal: AbortSignal) {
    const { context } = this;
    // An edit submitted during the rejected write supersedes it.
    const attempt = { draft: context.pending ?? rejected.model };
    context.pending = null;
    context.parameters.restore(rejected.parameters);
    try {
      await this.rebaseUntilApplied(rejected, attempt, signal);
    } catch (error) {
      // Recovery keeps a pending draft instead of restoring the engine document over it.
      if (!signal.aborted) context.pending ??= attempt.draft;
      throw error;
    }
  }

  private async rebaseUntilApplied(
    rejected: RejectedEdit<Model>,
    attempt: { draft: Model },
    signal: AbortSignal,
  ) {
    const { context } = this;
    const graphId = context.graphId();
    const deadline = context.runtime.clock.now() + context.timeoutMs;
    let baseline = rejected.before;
    while (true) {
      const current = await context.readEngine(signal, true, graphId ? { graphId } : undefined);
      // An edit submitted during the read, the backoff or the last write supersedes the draft.
      const rebased = rejected.rebase(baseline, current, context.pending ?? attempt.draft);
      context.guardOwner(signal, graphId);
      context.pending = null;
      attempt.draft = rebased;
      const parameters = context.parameters.take();
      try {
        await this.apply(rebased, [...parameters.values()], signal);
        return;
      } catch (error) {
        if (signal.aborted) throw error;
        context.parameters.restore(parameters);
        requireRebasableConflict(error, deadline, context.runtime.clock.now());
        baseline = current;
        await pauseGraphObservation(signal, context.runtime.scheduler);
      }
    }
  }
}
