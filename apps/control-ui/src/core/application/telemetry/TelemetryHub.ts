import type { ControlStateSnapshot, ControlTelemetry } from '../../domain/control/state';
import type { MeterTelemetryListener, StateListener } from '../../ports/control/TelemetryPort';
import type { Cancel, Scheduler } from '../../ports/Scheduler';

/** A stream that sends nothing for this long is treated as ended. */
export const TELEMETRY_SILENCE_MS = 2000;
/** State consumers reopen an ended stream after this delay. */
export const TELEMETRY_RETRY_MS = 5000;

type Listener = Partial<MeterTelemetryListener> & { state?: StateListener };

/** Receives one stream's packets until it ends. */
export interface TelemetryStreamListener {
  window(packet: ControlTelemetry): void;
  ended(): void;
}

/** Opens one telemetry stream; resolves to its close function once it delivers. */
export type TelemetrySource = (listener: TelemetryStreamListener) => Promise<() => void>;

/** One native channel/HTTP stream per port. Every consumer shares its attempt fence,
 * incarnation, liveness and complete state cache; old reads never refresh this cache. */
export class TelemetryHub {
  private listeners = new Set<Listener>();
  private opening?: Promise<void>;
  private close?: () => void;
  private attempt = 0;
  private incarnation?: string;
  private snapshot?: ControlStateSnapshot;
  private watchdog?: Cancel;
  private retry?: Cancel;

  constructor(
    private readonly source: TelemetrySource,
    private readonly scheduler: Scheduler,
  ) {}

  current() {
    return this.snapshot;
  }

  subscribeState(state: StateListener) {
    const listener = { state };
    this.listeners.add(listener);
    state(this.snapshot, this.incarnation);
    void this.start().catch(() => undefined);
    return () => this.remove(listener);
  }

  async subscribeMeters(listener: MeterTelemetryListener) {
    this.listeners.add(listener);
    try {
      await this.start();
    } catch (error) {
      this.remove(listener);
      throw error;
    }
    return () => this.remove(listener);
  }

  private remove(listener: Listener) {
    this.listeners.delete(listener);
    if (this.listeners.size) return;
    this.attempt += 1;
    this.close?.();
    this.close = undefined;
    this.opening = undefined;
    this.retry?.();
    this.watchdog?.();
    this.snapshot = undefined;
  }

  private receive(packet: ControlTelemetry) {
    if (packet.engineIncarnation !== this.incarnation) {
      this.incarnation = packet.engineIncarnation;
      this.snapshot = undefined;
      for (const listener of this.listeners) listener.state?.(undefined, this.incarnation);
    }
    if (packet.state && (!this.snapshot || packet.state.generation > this.snapshot.generation)) {
      this.snapshot = packet.state;
      for (const listener of this.listeners)
        listener.state?.(packet.state, packet.engineIncarnation);
    }
    for (const listener of this.listeners) listener.window?.(packet);
    this.watchdog?.();
    this.watchdog = this.scheduler.after(TELEMETRY_SILENCE_MS, () => this.failed());
  }

  /** Meter consumers are ended and must subscribe again; state consumers stay and see the
   * stream reopened. `notifyMeters` is false when the open itself was refused, because
   * `subscribeMeters` reports that rejection instead. */
  private failed(notifyMeters = true) {
    this.attempt += 1;
    this.close?.();
    this.close = undefined;
    this.opening = undefined;
    this.snapshot = undefined;
    this.watchdog?.();
    for (const listener of [...this.listeners]) {
      listener.state?.(undefined, this.incarnation);
      if (listener.window) {
        this.listeners.delete(listener);
        if (notifyMeters) listener.ended?.();
      }
    }
    this.retry?.();
    if (this.listeners.size)
      this.retry = this.scheduler.after(TELEMETRY_RETRY_MS, () => {
        void this.start().catch(() => undefined);
      });
  }

  private start(): Promise<void> {
    if (this.close) return Promise.resolve();
    if (this.opening) return this.opening;
    const attempt = ++this.attempt;
    const current = () => attempt === this.attempt;
    this.opening = this.source({
      window: (packet) => {
        if (current()) this.receive(packet);
      },
      ended: () => {
        if (current()) this.failed();
      },
    }).then(
      (close) => {
        if (!current()) {
          close();
          throw new Error('telemetry attempt ended');
        }
        this.close = close;
        this.opening = undefined;
      },
      (error: unknown) => {
        if (current()) this.failed(false);
        throw error;
      },
    );
    return this.opening;
  }
}
