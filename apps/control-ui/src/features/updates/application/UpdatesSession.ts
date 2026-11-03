import { repeat } from '../../../core/application/scheduling';
import type { ProductUpdate, UpdateProduct } from '../../../core/domain/updates';
import type { Cancel, Scheduler } from '../../../core/ports/Scheduler';
import type { UpdatesPort } from '../../../core/ports/UpdatesPort';

const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1_000;

interface UpdateState {
  products: ProductUpdate[];
  checked: boolean;
  checking: boolean;
  failed: boolean;
  opening: string | null;
  openFailed: boolean;
}

/** One port owner serializes update checks and installer-link opens. */
export class UpdatesSession {
  private state: UpdateState = {
    products: [],
    checked: false,
    checking: false,
    failed: false,
    opening: null,
    openFailed: false,
  };
  private active = false;
  private generation = 0;
  private cancel?: Cancel;
  private readonly listeners = new Set<() => void>();

  constructor(
    private readonly port: UpdatesPort | null,
    private readonly scheduler: Scheduler,
  ) {}

  snapshot = () => this.state;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  start() {
    this.active = true;
    const generation = ++this.generation;
    this.patch({
      products: [],
      checked: false,
      checking: false,
      failed: false,
      opening: null,
      openFailed: false,
    });
    if (!this.current(generation) || !this.port) return;
    // Effect replay retires the first owner before native work starts.
    void Promise.resolve().then(() => {
      if (this.current(generation)) void this.refresh();
    });
    this.cancel = repeat(this.scheduler, CHECK_INTERVAL_MS, () => {
      void this.refresh();
    });
  }

  dispose = () => {
    this.active = false;
    this.generation += 1;
    this.cancel?.();
  };

  private current(generation: number) {
    return this.active && generation === this.generation;
  }
  private patch(update: Partial<UpdateState>) {
    this.state = { ...this.state, ...update };
    for (const listener of this.listeners) listener();
  }

  refresh = async () => {
    if (!this.active || !this.port || this.state.checking || this.state.opening) return;
    const generation = this.generation;
    this.patch({ checking: true, openFailed: false });
    if (!this.current(generation)) return;
    try {
      const products = await this.port.check();
      if (this.current(generation)) this.patch({ products, checked: true, failed: false });
    } catch {
      if (this.current(generation))
        this.patch({
          products: this.state.products.map((product) => ({
            ...product,
            state: 'unavailable',
            latestVersion: null,
            downloads: [],
            notesUrl: null,
          })),
          checked: true,
          failed: true,
        });
    } finally {
      if (this.current(generation)) this.patch({ checking: false });
    }
  };

  openDownload = async (product: UpdateProduct, url: string) => {
    if (!this.active || !this.port || this.state.opening || this.state.checking) return;
    const generation = this.generation;
    this.patch({ opening: url, openFailed: false });
    if (!this.current(generation)) return;
    try {
      await this.port.openDownload(product, url);
    } catch {
      if (this.current(generation)) this.patch({ openFailed: true });
    } finally {
      if (this.current(generation)) this.patch({ opening: null });
    }
  };
}
