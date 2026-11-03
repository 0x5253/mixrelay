import type { DeviceAppActivity } from '../../../core/domain/control/inventory';
import type { InventoryPort } from '../../../core/ports/control/InventoryPort';
import type { Cancel, Scheduler } from '../../../core/ports/Scheduler';

export type DeviceActivityObservation =
  | { state: 'loading' | 'unavailable' }
  | { state: 'observed'; devices: Record<string, DeviceAppActivity[]> };
type ActivityPort = Pick<InventoryPort, 'getVirtualDeviceAppActivity'>;

/** Port/mapping and visibility lifetimes each fence late native replies. */
export class DeviceActivitySession {
  private active = true;
  private generation = 0;
  private running = false;
  private visible = false;
  private visibilityObserved = false;
  private timer?: Cancel;
  private expiry?: Cancel;

  constructor(
    private readonly port: ActivityPort,
    private readonly read: NonNullable<ActivityPort['getVirtualDeviceAppActivity']>,
    private readonly publish: (value: DeviceActivityObservation) => void,
    private readonly scheduler: Scheduler,
  ) {
    publish({ state: 'loading' });
  }

  private expire() {
    this.expiry?.();
    this.expiry = this.scheduler.after(3000, () => this.publish({ state: 'unavailable' }));
  }

  refresh = () => {
    void this.poll();
  };

  private async poll() {
    if (!this.active || this.running || !this.visible) return;
    this.timer?.();
    this.running = true;
    const request = this.generation;
    const current = () => this.active && request === this.generation;
    try {
      const devices = await this.read.call(this.port);
      if (current()) {
        this.publish({ state: 'observed', devices });
        this.expire();
      }
    } catch {
      if (current()) this.publish({ state: 'unavailable' });
    } finally {
      if (current()) {
        this.running = false;
        this.timer = this.scheduler.after(1000, this.refresh);
      }
    }
  }

  setVisible(visible: boolean) {
    const first = !this.visibilityObserved;
    this.visibilityObserved = true;
    this.generation += 1;
    this.running = false;
    this.visible = visible;
    this.timer?.();
    this.expiry?.();
    if (!first) this.publish({ state: 'unavailable' });
    if (visible || first) {
      this.expire();
      this.refresh();
    }
  }

  dispose() {
    this.active = false;
    this.timer?.();
    this.expiry?.();
  }
}
