import type { GraphLayout, GraphLayoutResponse } from '../../domain/control/catalog';
import { ControlPortError, hasErrorCode } from '../../domain/control/errors';
import { rebaseGraphLayout } from '../../domain/graph/rebaseGraphLayout';
import type { WorkspacePort } from '../../ports/control/WorkspacePort';
import type { Cancel, MonotonicClock, Scheduler } from '../../ports/Scheduler';
import type { LayoutNodeProjection } from '../../ports/WorkspaceRuntime';

/** Layout reads and writes, fenced by the catalog's active graph. */
export type LayoutPort = Pick<
  WorkspacePort,
  'getGraphCatalog' | 'getGraphLayout' | 'setGraphLayout'
>;

export interface LayoutState {
  layout: GraphLayout;
  loaded: boolean;
  loading: boolean;
  saving: boolean;
  dirty: boolean;
  error: boolean;
  restoreVersion: number;
}

function bounded<T>(request: Promise<T>, scheduler: Scheduler): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = scheduler.after(10_000, () => reject(new Error('layout request timed out')));
    request.then(
      (value) => {
        timer?.();
        resolve(value);
      },
      (error) => {
        timer?.();
        reject(error);
      },
    );
  });
}

/** Graph-scoped single writer. Conflicts never silently replace another window's layout. */
export class GraphLayoutController {
  private state: LayoutState;
  private revision = 0;
  private base: GraphLayout = { positions: {} };
  private generation = 0;
  private disposed = false;
  private timer: Cancel | null = null;
  private cancelRecoveryPause: (() => void) | null = null;
  private readonly listeners = new Set<() => void>();
  readonly enabled: boolean;
  private readonly graphId: string;

  constructor(
    private readonly port: LayoutPort,
    graphId: string | undefined,
    private readonly scheduler: Scheduler,
    private readonly clock: MonotonicClock,
    private readonly projection: LayoutNodeProjection,
  ) {
    // Without a graph there is no layout to read or save.
    this.graphId = graphId ?? '';
    this.enabled = this.graphId !== '';
    this.state = {
      layout: { positions: {} },
      loaded: !this.enabled,
      loading: false,
      saving: false,
      dirty: false,
      error: false,
      restoreVersion: 0,
    };
  }

  private bounded<T>(request: Promise<T>) {
    return bounded(request, this.scheduler);
  }

  snapshot = () => this.state;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  private patch(update: Partial<LayoutState>) {
    this.state = { ...this.state, ...update };
    for (const listener of this.listeners) listener();
  }
  private cancelTimer() {
    if (this.timer) this.timer?.();
    this.timer = null;
  }
  dispose = () => {
    this.disposed = true;
    this.cancelTimer();
    this.cancelRecoveryPause?.();
    if (this.state.loading) this.generation++;
    // Switching graphs must not discard the final drag/resize debounce. An in-flight
    // writer finishes before save() submits the latest dirty layout at its new revision.
    else if (!this.state.error) void this.save();
  };

  private validate(response: GraphLayoutResponse) {
    if (response.graphId !== this.graphId) throw new Error('layout graph identity mismatch');
    return response;
  }

  /** Explicit reload discards only presentation edits, never audio graph/Undo state. */
  reload = async () => {
    if (!this.enabled) return;
    // React StrictMode may set up this same owner again after its cleanup probe.
    this.disposed = false;
    this.cancelTimer();
    this.cancelRecoveryPause?.();
    const generation = ++this.generation;
    this.patch({ loading: true, saving: false, error: false });
    try {
      const response = this.validate(await this.bounded(this.port.getGraphLayout(this.graphId)));
      if (generation !== this.generation) return;
      this.revision = response.layoutRevision;
      this.base = response.layout;
      this.patch({
        layout: response.layout,
        loaded: true,
        dirty: false,
        restoreVersion: this.state.restoreVersion + 1,
      });
    } catch {
      if (generation === this.generation) this.patch({ error: true });
    } finally {
      if (generation === this.generation) this.patch({ loading: false });
    }
  };

  update = (layout: GraphLayout) => {
    if (this.disposed || !this.enabled || !this.state.loaded || this.state.loading) return;
    if (JSON.stringify(layout) === JSON.stringify(this.state.layout)) return;
    this.patch({ layout, dirty: true });
    this.cancelTimer();
    if (!this.state.error)
      this.timer = this.scheduler.after(400, () => {
        void this.save();
      });
  };

  private definiteConflict(error: unknown) {
    if (!(error instanceof ControlPortError) || !hasErrorCode(error, 'graph_revision_stale'))
      return false;
    const parameters = error.body.parameters;
    return Boolean(
      parameters &&
        typeof parameters === 'object' &&
        'resource' in parameters &&
        parameters.resource === 'graph_layout' &&
        'graphId' in parameters &&
        parameters.graphId === this.graphId,
    );
  }

  private async rebase(generation: number) {
    const catalog = await this.bounded(this.port.getGraphCatalog());
    if (generation !== this.generation) return false;
    if (this.disposed) throw new Error('layout owner disposed');
    if (catalog.activeId !== this.graphId) throw new Error('layout graph identity mismatch');
    const response = this.validate(
      catalog.layout && catalog.layoutRevision !== undefined
        ? {
            graphId: catalog.activeId,
            layoutRevision: catalog.layoutRevision,
            layout: catalog.layout,
          }
        : await this.bounded(this.port.getGraphLayout(this.graphId)),
    );
    if (generation !== this.generation) return false;
    if (this.disposed) throw new Error('layout owner disposed');
    const nodeIds = this.projection.nodeIds(catalog.document);
    const layout = rebaseGraphLayout(this.base, this.state.layout, response.layout, nodeIds);
    this.revision = response.layoutRevision;
    this.base = response.layout;
    this.patch({ layout, error: false, restoreVersion: this.state.restoreVersion + 1 });
    if (generation !== this.generation) return false;
    if (this.disposed) throw new Error('layout owner disposed');
    return true;
  }

  private async write(generation: number, graphId: string) {
    if (generation !== this.generation || this.state.loading) return;
    const layout = this.state.layout;
    const response = this.validate(
      await this.bounded(
        this.port.setGraphLayout(graphId, {
          expectedLayoutRevision: this.revision,
          layout,
        }),
      ),
    );
    if (generation !== this.generation) return;
    this.revision = response.layoutRevision;
    this.base = response.layout;
    this.patch({ dirty: this.state.layout !== layout, error: false });
  }

  private async saveAttempt(generation: number, graphId: string) {
    const deadline = this.clock.now() + 10_000;
    let retried = false;
    while (true) {
      try {
        await this.write(generation, graphId);
        return;
      } catch (error) {
        if (!(await this.recoverRejectedWrite(error, generation, deadline, retried))) return;
        retried = true;
      }
    }
  }

  private async recoverRejectedWrite(
    error: unknown,
    generation: number,
    deadline: number,
    retried: boolean,
  ) {
    if (generation !== this.generation) return false;
    // Only a definite CAS rejection proves this write applied nothing. Merge
    // ordinary concurrent geometry; unknown acknowledgements require explicit retry.
    if (!this.definiteConflict(error) || this.clock.now() >= deadline) throw error;
    if (retried && !(await this.pauseRecovery())) {
      if (this.disposed) throw new Error('layout owner disposed');
      return false;
    }
    return this.rebase(generation);
  }

  private pauseRecovery(): Promise<boolean> {
    return new Promise((resolve) => {
      const settle = (ready: boolean) => {
        timer?.();
        this.cancelRecoveryPause = null;
        resolve(ready);
      };
      const timer = this.scheduler.after(200, () => settle(true));
      this.cancelRecoveryPause = () => settle(false);
    });
  }

  private async save() {
    this.cancelTimer();
    const graphId = this.graphId;
    if (!this.enabled || this.state.saving || this.state.loading || !this.state.dirty) return;
    const generation = this.generation;
    this.patch({ saving: true });
    try {
      await this.saveAttempt(generation, graphId);
    } catch {
      if (generation === this.generation) this.patch({ error: true });
    } finally {
      if (generation === this.generation) {
        this.patch({ saving: false });
        if (this.state.dirty && !this.state.error) void this.save();
      }
    }
  }

  /** Explicit retry uses the same field merge, preserving other writers' geometry. */
  retry = async () => {
    if (this.disposed || this.state.loading || this.state.saving) return;
    // Only an enabled controller accepts edits, so a dirty one has a graph.
    if (!this.state.dirty) return this.reload();
    const generation = ++this.generation;
    this.patch({ loading: true });
    try {
      await this.rebase(generation);
    } catch {
      if (generation === this.generation) this.patch({ error: true });
    } finally {
      if (generation === this.generation) {
        this.patch({ loading: false });
        if (!this.state.error) void this.save();
      }
    }
  };
}
