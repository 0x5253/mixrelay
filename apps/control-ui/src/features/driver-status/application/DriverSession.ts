import type { DriverAction, DriverSnapshot } from '../../../core/domain/driver';
import type { DriverPort } from '../../../core/ports/DriverPort';
import type { Cancel, Scheduler } from '../../../core/ports/Scheduler';
import { configuredDevice, type EndpointConfiguration } from '../domain/deviceControl';

type DriverFailure = 'unavailable' | 'deviceMissing' | 'saveFailed' | 'changeFailed';
interface DriverState {
  snapshot: DriverSnapshot | null;
  busy: boolean;
  error: DriverFailure | null;
  endpointId: string | null;
}

/** Native snapshots authorize commands; a replacement owner never accepts old replies. */
export class DriverSession {
  private state: DriverState = { snapshot: null, busy: false, error: null, endpointId: null };
  private readonly listeners = new Set<() => void>();
  private active = false;
  private generation = 0;
  private timer?: Cancel;

  constructor(
    private readonly port: DriverPort | null,
    private readonly scheduler: Scheduler,
  ) {}

  snapshot = () => this.state;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  private patch(update: Partial<DriverState>) {
    this.state = { ...this.state, ...update };
    for (const listener of this.listeners) listener();
  }
  private current(generation: number) {
    return this.active && generation === this.generation;
  }

  start() {
    this.active = true;
    this.patch({ snapshot: null, busy: false, error: null, endpointId: null });
    const generation = ++this.generation;
    if (!this.port) return;
    const poll = async () => {
      await this.refresh();
      if (this.current(generation))
        this.timer = this.scheduler.after(5000, () => {
          void poll();
        });
    };
    void Promise.resolve().then(() => {
      if (this.current(generation)) void poll();
    });
  }
  dispose = () => {
    this.active = false;
    this.generation += 1;
    this.timer?.();
  };

  refresh = async () => {
    if (!this.active || !this.port || this.state.busy) return;
    const generation = this.generation;
    this.patch({ busy: true });
    if (!this.current(generation)) return;
    try {
      const snapshot = await this.port.getSnapshot();
      if (this.current(generation))
        this.patch({ snapshot, error: this.state.endpointId ? this.state.error : null });
    } catch {
      if (this.current(generation)) this.patch({ snapshot: null, error: 'unavailable' });
    } finally {
      if (this.current(generation)) this.patch({ busy: false });
    }
  };

  private mutationTarget(action: DriverAction) {
    const snapshot = this.state.snapshot;
    if (!this.active || this.state.busy || snapshot?.state !== 'ready') return null;
    const endpointId = 'endpointId' in action ? action.endpointId : action.spec.id;
    if (
      action.action !== 'create' &&
      !snapshot.settings.devices.some((device) => device.spec.id === endpointId)
    ) {
      this.patch({ error: 'deviceMissing', endpointId });
      return null;
    }
    return { snapshot, endpointId };
  }

  mutate = async (action: DriverAction): Promise<DriverSnapshot | null> => {
    const target = this.mutationTarget(action);
    if (!target || !this.port) return null;
    const { snapshot: previous, endpointId } = target;
    const generation = this.generation;
    this.patch({ busy: true, error: null, endpointId });
    if (!this.current(generation)) return null;
    try {
      const snapshot = await this.port.mutate({ ...action, expectedRevision: previous.revision });
      if (!this.current(generation)) return null;
      this.patch({ snapshot, error: snapshot.persistenceError ? 'saveFailed' : null });
      return snapshot;
    } catch {
      if (this.current(generation)) this.patch({ error: 'changeFailed' });
      return null;
    } finally {
      if (this.current(generation)) this.patch({ busy: false });
    }
  };

  changeDevice = async (endpointId: string, update: EndpointConfiguration) => {
    const device = this.state.snapshot?.settings.devices.find(
      (candidate) => candidate.spec.id === endpointId,
    );
    if (!device) {
      this.patch({ error: 'deviceMissing', endpointId });
      return null;
    }
    const action: DriverAction = update.sampleFormat
      ? { action: 'set_sample_format', endpointId, sampleFormat: update.sampleFormat }
      : configuredDevice(device.spec, update);
    return this.mutate(action);
  };
}
