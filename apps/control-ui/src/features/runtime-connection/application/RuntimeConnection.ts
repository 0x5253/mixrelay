import type { RuntimeConnectStage, RuntimeStatus } from '../../../core/domain/runtime';
import type { RuntimePort } from '../../../core/ports/RuntimePort';
import type { Cancel, Scheduler } from '../../../core/ports/Scheduler';

interface ConnectionState {
  state: RuntimeStatus | 'connecting';
  stage: RuntimeConnectStage;
  opened: boolean;
}

/** Recovery observes health without repeating a launch the user may have stopped. */
export class RuntimeConnection {
  private state: ConnectionState = { state: 'connecting', stage: 'checking', opened: false };
  private readonly listeners = new Set<() => void>();
  private active = false;
  private generation = 0;
  private timer?: Cancel;
  private port: RuntimePort | null = null;

  constructor(private readonly scheduler: Scheduler) {}
  snapshot = () => this.state;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  private patch(update: Partial<ConnectionState>) {
    this.state = { ...this.state, ...update };
    for (const listener of this.listeners) listener();
  }
  private current(generation: number) {
    return this.active && generation === this.generation;
  }
  start(port: RuntimePort) {
    this.port = port;
    this.active = true;
    void this.connect();
  }
  dispose = () => {
    this.active = false;
    this.generation += 1;
    this.timer?.();
  };

  connect = async () => {
    const port = this.port;
    if (!this.active || !port) return;
    this.timer?.();
    const generation = ++this.generation;
    this.patch({ state: 'connecting', stage: 'checking' });
    if (!this.current(generation)) return;
    let state: RuntimeStatus;
    try {
      state = await port.connect((stage) => {
        if (this.current(generation)) this.patch({ stage });
      });
    } catch {
      state = 'unavailable';
    }
    if (!this.current(generation)) return;
    this.accept(state);
    this.schedule(port, generation);
  };
  private accept(state: RuntimeStatus) {
    this.patch({ state, opened: this.state.opened || state === 'ready' });
  }
  private schedule(port: RuntimePort, generation: number) {
    if (!this.current(generation)) return;
    if (this.state.state !== 'ready' && this.state.state !== 'unavailable') return;
    this.timer = this.scheduler.after(2000, () => {
      void this.observe(port, generation);
    });
  }
  private async observe(port: RuntimePort, generation: number) {
    if (!this.current(generation)) return;
    let state: RuntimeStatus;
    try {
      state = await port.status();
    } catch {
      state = 'unavailable';
    }
    if (!this.current(generation)) return;
    this.accept(state);
    this.schedule(port, generation);
  }
}
