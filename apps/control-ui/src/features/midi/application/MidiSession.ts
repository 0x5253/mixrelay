import type { MidiStatus } from '../../../core/domain/midi';
import type { MidiCommand, MidiPort } from '../../../core/ports/MidiPort';
import type { Cancel, Scheduler } from '../../../core/ports/Scheduler';

export interface MidiState {
  status: MidiStatus | null;
  busy: boolean;
  error: boolean;
}

/** Only the newest command and its current open session may publish MIDI state. */
export class MidiSession {
  private state: MidiState = { status: null, busy: false, error: false };
  private active = false;
  private generation = 0;
  private requests = 0;
  private pending: number | null = null;
  private timer?: Cancel;

  constructor(
    private readonly port: MidiPort,
    private readonly scheduler: Scheduler,
    private readonly publish: (state: MidiState) => void,
  ) {}
  private patch(update: Partial<MidiState>) {
    this.state = { ...this.state, ...update };
    this.publish(this.state);
  }
  private current(owner: number, request: number) {
    return this.active && owner === this.generation && request === this.requests;
  }

  start(opened: boolean) {
    this.active = true;
    const owner = ++this.generation;
    this.timer?.();
    this.pending = null;
    this.patch({ status: null, busy: false, error: false });
    if (!opened) return;
    const poll = async () => {
      const request = this.requests;
      const mutating = this.pending !== null;
      try {
        const status = await this.port.command('midi_status');
        if (this.current(owner, request) && !mutating) this.patch({ status });
      } catch {
        if (this.current(owner, request) && !mutating) this.patch({ error: true });
      } finally {
        if (this.active && owner === this.generation)
          this.timer = this.scheduler.after(250, () => {
            void poll();
          });
      }
    };
    void poll();
  }
  dispose = () => {
    this.active = false;
    this.generation += 1;
    this.timer?.();
  };

  command = async (name: MidiCommand, args?: Record<string, unknown>) => {
    if (!this.active) return false;
    const owner = this.generation;
    const request = ++this.requests;
    this.pending = request;
    this.patch({ busy: true, error: false });
    try {
      const status = await this.port.command(name, args);
      if (this.current(owner, request)) this.patch({ status });
      return this.current(owner, request);
    } catch {
      if (this.current(owner, request)) this.patch({ error: true });
      return false;
    } finally {
      if (this.current(owner, request)) {
        this.pending = null;
        this.patch({ busy: false });
      }
    }
  };
}
