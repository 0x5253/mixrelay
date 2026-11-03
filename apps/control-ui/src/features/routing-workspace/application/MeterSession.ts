import type { MeterSnapshot, MeterTelemetry } from '../../../core/domain/control/meters';
import type { ControlPort } from '../../../core/ports/control/ControlPort';
import type { Cancel, MonotonicClock, Scheduler } from '../../../core/ports/Scheduler';

export interface MeterOutput {
  update(snapshot: MeterSnapshot): void;
  clear(): void;
}
const POLL_MS = 50;
const RETRY_MS = 500;
/** How often a session that fell back to polling tries push again. */
const PUSH_RETRY_MS = 5000;
const STALE_MS = 250;
type MeterPort = Pick<ControlPort, 'getMeters' | 'subscribeMeters'>;

/** Effect replay reuses the original native request and its age, never a new request. */
export class MeterRequestOwner {
  private pending?: { promise: Promise<MeterSnapshot>; started: number };

  constructor(
    private port: MeterPort,
    private clock: MonotonicClock,
  ) {}

  read() {
    if (this.pending) return this.pending;
    const request = {
      started: this.clock.now(),
      promise: Promise.resolve().then(() => this.port.getMeters()),
    };
    this.pending = request;
    const release = () => {
      // read() reuses this owner until settlement, so this is its only pending request.
      this.pending = undefined;
    };
    void request.promise.then(release, release);
    return request;
  }
}

/** Meters while the window is visible: pushed windows where the port offers them, otherwise
 * bounded polling. A refused subscription polls and retries push in the background, so an
 * engine that was down at first still ends up pushed. A freshness watchdog clears stalled
 * telemetry without starting unbounded native requests, and windows from a replaced engine
 * never light up its successor. */
export class MeterSession {
  private disposed = false;
  private pending = false;
  /** Polling continues only while this is set; a working subscription clears it. */
  private polling = false;
  /** Replies from an earlier polling phase never overwrite or clear pushed meters. */
  private pollGeneration = 0;
  /** Each subscription attempt; a later attempt, an end or a stop retires the earlier one. */
  private attempt = 0;
  private pollTimer?: Cancel;
  private pushTimer?: Cancel;
  private staleTimer?: Cancel;
  private unsubscribe?: () => void;
  private incarnation?: string;

  constructor(
    private port: MeterPort,
    private requests: MeterRequestOwner,
    private store: MeterOutput,
    private scheduler: Scheduler,
    private clock: MonotonicClock,
  ) {}

  private visible = false;

  setVisible(visible: boolean) {
    this.visible = visible;
    this.stop();
    this.resume();
  }

  private resume() {
    if (this.disposed || !this.visible) return;
    if (this.port.subscribeMeters) void this.subscribe();
    else this.startPolling();
  }

  private clear() {
    this.staleTimer?.();
    this.store.clear();
  }

  private stop() {
    this.attempt += 1;
    this.stopPolling();
    this.pushTimer?.();
    this.unsubscribe?.();
    this.unsubscribe = undefined;
    this.clear();
  }

  private publish(snapshot: MeterSnapshot) {
    this.store.update(snapshot);
    this.staleTimer?.();
    this.staleTimer = this.scheduler.after(STALE_MS, () => this.clear());
  }

  private async subscribe() {
    this.attempt += 1;
    const attempt = this.attempt;
    const current = () => !this.disposed && attempt === this.attempt;
    try {
      const unsubscribe = await this.port.subscribeMeters?.({
        window: (telemetry) => {
          if (!current()) return;
          // Windows can precede the subscription's resolution; push owns the store from now.
          if (this.polling) this.stopPolling();
          this.receive(telemetry);
        },
        ended: () => {
          if (!current()) return;
          // Retires this attempt, so its late resolution cannot replace a successor.
          this.attempt += 1;
          this.unsubscribe = undefined;
          this.clear();
          this.pushTimer = this.scheduler.after(RETRY_MS, () => this.resume());
        },
      });
      if (!current()) return unsubscribe?.();
      this.unsubscribe = unsubscribe;
      this.stopPolling();
    } catch {
      if (!current()) return;
      this.startPolling();
      this.pushTimer = this.scheduler.after(PUSH_RETRY_MS, () => this.resume());
    }
  }

  private startPolling() {
    if (this.polling) return;
    this.polling = true;
    this.pollGeneration += 1;
    void this.poll();
  }

  private stopPolling() {
    this.pollTimer?.();
    this.polling = false;
    this.pollGeneration += 1;
  }

  private receive(telemetry: MeterTelemetry) {
    if (telemetry.engineIncarnation !== this.incarnation) {
      this.incarnation = telemetry.engineIncarnation;
      this.store.clear();
    }
    this.publish(telemetry.meters);
  }

  private async poll() {
    if (this.disposed || this.pending || !this.polling) return;
    this.pending = true;
    const generation = this.pollGeneration;
    const current = () => !this.disposed && generation === this.pollGeneration;
    const request = this.requests.read();
    let delay = POLL_MS;
    try {
      const snapshot = await request.promise;
      if (!current()) return;
      if (this.clock.now() - request.started >= STALE_MS) {
        this.clear();
        return;
      }
      this.publish(snapshot);
    } catch {
      if (current()) this.clear();
      delay = RETRY_MS;
    } finally {
      this.pending = false;
      // Continues the current phase too: its own first poll yielded to this pending one.
      if (this.polling && !this.disposed && this.visible) {
        this.pollTimer = this.scheduler.after(delay, () => void this.poll());
      }
    }
  }

  dispose() {
    this.disposed = true;
    this.stop();
  }
}
