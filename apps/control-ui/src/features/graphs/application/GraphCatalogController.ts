import type { GraphCatalog, GraphCatalogAction } from '../../../core/domain/control/catalog';
import type { RuntimeGraphDocument } from '../../../core/domain/control/graph';
import type { GraphPort } from '../../../core/ports/control/GraphPort';
import type { WorkspacePort } from '../../../core/ports/control/WorkspacePort';
import type { Cancel, MonotonicClock, Scheduler } from '../../../core/ports/Scheduler';
import {
  errorCode,
  mutationUncertain,
  retryableRead,
  workspaceUncertain,
} from '../domain/catalogFailure';

/** The catalog, its workspace files and the graph activation they wait for. */
export type CatalogPort = Pick<GraphPort, 'getGraph' | 'getGraphStatus'> &
  Pick<WorkspacePort, 'getGraphCatalog' | 'mutateGraphCatalog' | 'workspaceFile'>;

export interface GraphCatalogState {
  catalog: GraphCatalog | null;
  busy: boolean;
  error: string | null;
  uncertain: boolean;
  /** Only document exchanges change this; renaming must not reset graph undo history. */
  documentGeneration: number;
  document?: RuntimeGraphDocument;
}

function bounded<T>(request: Promise<T>, scheduler: Scheduler): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = scheduler.after(10_000, () => reject(new Error('catalog request timed out')));
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

/** One writer, confirmed audio activation, and generation fencing for late IPC replies. */
export class GraphCatalogController {
  private state: GraphCatalogState = {
    catalog: null,
    busy: false,
    error: null,
    uncertain: false,
    documentGeneration: 0,
  };
  private generation = 0;
  private mutationSent = false;
  private retryTimer: Cancel | undefined;
  private retryDelay = 500;
  private readonly listeners = new Set<() => void>();

  constructor(
    private readonly port: CatalogPort,
    private readonly scheduler: Scheduler,
    private readonly clock: MonotonicClock,
  ) {}

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
  dispose = () => {
    this.generation += 1;
    this.retryTimer?.();
    this.patch({ busy: false });
  };
  clearError = () => this.patch({ error: null });

  private patch(update: Partial<GraphCatalogState>) {
    this.state = { ...this.state, ...update };
    for (const listener of this.listeners) listener();
  }

  private async run(work: (generation: number) => Promise<void>, retryRead = false) {
    if (this.state.busy) return;
    this.retryTimer?.();
    const generation = ++this.generation;
    this.mutationSent = false;
    this.patch({ busy: true, error: null });
    try {
      await work(generation);
    } catch (error) {
      if (generation !== this.generation) return;
      this.patch({
        error: errorCode(error),
        uncertain: this.state.uncertain || (this.mutationSent && mutationUncertain(error)),
      });
      if (retryRead && !this.state.catalog && retryableRead(error)) {
        this.retryTimer = this.scheduler.after(this.retryDelay, () => {
          // Every operation and disposal clears this timer before changing its generation.
          void this.reload();
        });
        this.retryDelay = Math.min(5000, this.retryDelay * 2);
      }
    } finally {
      if (generation === this.generation) this.patch({ busy: false });
    }
  }

  private accept(catalog: GraphCatalog, generation: number, replaceDocument: boolean) {
    if (generation !== this.generation) return;
    this.retryDelay = 500;
    this.patch({
      catalog,
      error: null,
      uncertain: false,
      document: replaceDocument ? catalog.document : this.state.document,
      documentGeneration: this.state.documentGeneration + (replaceDocument ? 1 : 0),
    });
  }

  reload = () =>
    this.run(async (generation) => {
      this.accept(await this.bounded(this.port.getGraphCatalog()), generation, true);
    }, true);

  workspaceFile = (action: 'new' | 'open' | 'save' | 'save_as', locale: string) =>
    this.run(async (generation) => {
      const catalog = this.state.catalog;
      if (!catalog || this.state.uncertain || !this.port.workspaceFile) return;
      const status = await this.bounded(this.port.getGraphStatus());
      if (generation !== this.generation) return;
      this.mutationSent = true;
      const next = await this.port
        .workspaceFile({
          action,
          locale,
          expectedRevision: status.submittedRevision,
          expectedCatalogRevision: catalog.catalogRevision,
        })
        .catch((error: unknown) => {
          if (generation === this.generation && workspaceUncertain(error))
            this.patch({ uncertain: true });
          throw error;
        });
      // A native file dialog can outlive this editor owner. Its reply must not
      // mutate the replacement view or begin another catalog synchronization.
      if (generation !== this.generation) return;
      if (!next) {
        this.mutationSent = false;
        return;
      }
      this.accept(next, generation, action === 'new' || action === 'open');
      await this.confirmWorkspaceFile(next, generation);
    });

  private async confirmWorkspaceFile(next: GraphCatalog, generation: number) {
    if (generation !== this.generation) return;
    this.patch({ uncertain: true });
    await this.waitActive(next.document.revision, generation);
    if (generation !== this.generation) return;
    // The accepted document remains visible even if capture synchronization fails.
    this.accept(await this.port.getGraphCatalog(), generation, false);
  }

  private async waitActive(revision: number, generation: number) {
    const deadline = this.clock.now() + 10_000;
    while (generation === this.generation) {
      const status = await this.bounded(this.port.getGraphStatus());
      if (status.submittedRevision !== revision) throw new Error('graph changed during activation');
      if (status.activeRevision === revision) return;
      if (this.clock.now() >= deadline) throw new Error('graph activation timed out');
      await new Promise<void>((resolve) => this.scheduler.after(100, resolve));
    }
  }

  mutate = async (action: GraphCatalogAction) => {
    const catalog = this.state.catalog;
    if (!catalog || this.state.uncertain) return;
    await this.run(async (generation) => {
      const document = await this.bounded(this.port.getGraph());
      if (generation !== this.generation) return;
      this.mutationSent = true;
      const next = await this.bounded(
        this.port.mutateGraphCatalog({
          ...action,
          expectedRevision: document.revision,
          expectedCatalogRevision: catalog.catalogRevision,
        }),
      );
      await this.waitActive(next.document.revision, generation);
      this.accept(next, generation, action.action !== 'rename');
    });
  };
}
