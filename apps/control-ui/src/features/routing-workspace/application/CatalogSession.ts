import type { ExternalProcessorCatalog } from '../../../core/domain/control/processors';
import type { ProcessorPaths } from '../../../core/ports/ProcessorPaths';
import type { CatalogState, ProcessorCatalogFailure } from '../domain/processorCatalog';
import type { CatalogAction, CatalogPort } from '../ports/ProcessorDiscovery';

/** One port owns one serialized discovery session. Disposal invalidates every native promise. */
export class CatalogSession {
  private cancelled = false;
  private running = false;
  private failedAction: CatalogAction = 'initial';
  private phase: ProcessorCatalogFailure = 'catalog';
  private paths: string[] = [];

  constructor(
    private port: CatalogPort,
    private pathsStore: ProcessorPaths | undefined,
    private update: (patch: Partial<CatalogState>) => void,
  ) {}

  private async preparePaths(action: CatalogAction): Promise<boolean> {
    if (action === 'initial') {
      this.phase = 'paths';
      this.paths = this.pathsStore?.read() ?? [];
    }
    if (action !== 'directory') return true;
    this.phase = 'directory';
    if (!this.port.selectProcessorDirectory) throw new Error('Directory picker unavailable');
    const selected = await this.port.selectProcessorDirectory();
    if (this.cancelled || !selected) return false;
    this.phase = 'paths';
    const next = [...new Set([...this.paths, selected])];
    this.pathsStore?.write(next);
    this.paths = next;
    return true;
  }

  private accept(catalog: ExternalProcessorCatalog) {
    if (this.cancelled) return;
    const diagnostics = catalog.diagnostics ?? [];
    // A GET can return the last failed discovery. Retry must execute a new scan.
    if (diagnostics.length) this.failedAction = 'scan';
    this.update({ processors: catalog.processors, diagnostics, failure: null });
  }

  async run(action = this.failedAction) {
    if (this.cancelled || this.running) return;
    this.running = true;
    this.failedAction = action;
    this.update({ busy: true });
    try {
      if (!(await this.preparePaths(action)) || this.cancelled) return;
      this.phase = 'catalog';
      if (action === 'directory') this.failedAction = 'scan';
      const catalog = await (action === 'initial' && !this.paths.length
        ? this.port.getProcessors()
        : this.port.rescanProcessors(this.paths));
      this.accept(catalog);
    } catch {
      if (!this.cancelled) this.update({ failure: this.phase });
    } finally {
      this.running = false;
      if (!this.cancelled) this.update({ busy: false });
    }
  }

  dispose() {
    this.cancelled = true;
  }
}
