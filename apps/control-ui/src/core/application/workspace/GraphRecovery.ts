import { hasErrorCode } from '../../domain/control/errors';
import type { RuntimeGraphDocument } from '../../domain/control/graph';
import { errorCode, failure } from '../../domain/graph/WorkspaceEdits';
import type { GraphWriterContext } from './GraphWriterContext';

/** How a definite revision rejection without a rebase contract was resolved. */
type ConflictRecovery = 'restored' | 'draft_kept' | 'unresolved';

export type RecoveryMode = 'retry' | 'reload' | 'refresh';

/** Blocks a failed writer on the engine baseline and reopens it by retry, reload or refresh. */
export class GraphRecovery<Model extends object> {
  constructor(private readonly context: GraphWriterContext<Model>) {}

  async recover(error: unknown, signal: AbortSignal, graphId?: string) {
    const { context } = this;
    if (signal.aborted || context.graphId() !== graphId) return;
    // Taken before a conflict read publishes the engine document: a retry rebases from here.
    const baseline = context.source();
    const conflict = context.pending
      ? 'unresolved'
      : await this.restoreRevisionConflict(error, signal, graphId);
    if (conflict === 'restored' || signal.aborted) return;
    context.blocked = true;
    context.recoveryBaseline ??= baseline;
    // Refresh the engine baseline, never overwrite the user's draft/history on a failed edit.
    // A failed read leaves the queue blocked; retry/reload must read it again before writing.
    try {
      await context.readEngine(signal, false, graphId ? { graphId } : undefined);
    } catch {
      if (signal.aborted) return;
    }
    if (signal.aborted) return;
    if (graphId && context.graphId() !== graphId) return;
    context.view.error(conflict === 'draft_kept' ? 'graph_draft_changed' : errorCode(error));
    context.view.state('blocked');
  }

  /** Reads the engine and prepares the writer for the explicit recovery `mode`. */
  async prepare(mode: RecoveryMode, signal: AbortSignal, initialModel: Model | null) {
    const { context } = this;
    if (mode === 'reload' && context.view.beforeReload)
      await context.readback.bound(context.view.beforeReload(), signal);
    const before = context.recoveryBaseline ?? context.source();
    const graphId = context.graphId();
    const document = await context.readEngine(
      signal,
      mode !== 'refresh',
      graphId ? { graphId } : undefined,
      mode === 'reload' ? 'deferred' : 'immediate',
    );
    if (mode === 'refresh') {
      this.acceptRefresh(before, document, signal, graphId);
      return;
    }
    if (mode === 'reload') await this.acceptReload(document, signal, graphId, initialModel);
    else this.rebaseForRetry(before, document, signal, graphId);
    context.blocked = false;
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
    const { context } = this;
    if (!hasErrorCode(error, 'graph_revision_stale')) return 'unresolved';
    try {
      const document = await context.readEngine(signal, true, graphId ? { graphId } : undefined);
      if (context.pending) return 'draft_kept';
      context.parameters.clear();
      context.view.restore(document);
      context.guardOwner(signal, graphId);
      context.blocked = false;
      context.view.error(null);
      context.view.state('active');
      return 'restored';
    } catch {
      return 'unresolved';
    }
  }

  private acceptRefresh(
    before: RuntimeGraphDocument | null,
    document: RuntimeGraphDocument,
    signal: AbortSignal,
    graphId: string | undefined,
  ) {
    const { context } = this;
    if (before && context.pending && context.view.rebase) {
      const rebased = context.view.rebase(before, document, context.pending);
      context.guardOwner(signal, graphId);
      context.pending = rebased;
    } else {
      if (context.pending) throw failure('graph_draft_changed');
      if (before) context.acceptControls(before, document, signal, graphId, true);
      else context.view.restore(document);
    }
    context.guardOwner(signal, graphId);
    context.blocked = false;
    context.recoveryBaseline = null;
  }

  private async acceptReload(
    document: RuntimeGraphDocument,
    signal: AbortSignal,
    graphId: string | undefined,
    initialModel: Model | null,
  ) {
    const { context } = this;
    const savedLayout = await this.readReloadLayout(graphId, signal);
    if (context.view.getModel() !== initialModel) throw failure('graph_draft_changed');
    context.pending = null;
    context.parameters.clear();
    // Publishing the document refreshes presentation and therefore the model identity.
    // Defer that side effect until every asynchronous read and the draft guard have finished.
    context.publish(document);
    context.guardOwner(signal, graphId);
    context.view.restore(document, savedLayout);
    context.guardOwner(signal, graphId);
    context.recoveryBaseline = null;
  }

  private rebaseForRetry(
    before: RuntimeGraphDocument | null,
    document: RuntimeGraphDocument,
    signal: AbortSignal,
    graphId: string | undefined,
  ) {
    const { context } = this;
    const local = context.view.getModel();
    const rebased =
      before && local && context.view.rebase ? context.view.rebase(before, document, local) : local;
    context.guardOwner(signal, graphId);
    context.pending = rebased;
    context.recoveryBaseline = document;
  }

  private async readReloadLayout(graphId: string | undefined, signal: AbortSignal) {
    const { context } = this;
    if (!graphId || !context.view.getReloadLayout) return undefined;
    const response = await context.readback.bound(context.view.getReloadLayout(graphId), signal);
    signal.throwIfAborted();
    if (response.graphId !== graphId) throw failure('graph_layout_identity_mismatch');
    if (context.graphId() !== graphId) throw failure('graph_revision_stale');
    return response.layout;
  }
}
