import type { MeterSnapshot, MeterTelemetry } from '../../domain/control/meters';
import type { ControlStateSnapshot } from '../../domain/control/state';

/** Receives pushed meter windows until the stream ends. */
export interface MeterTelemetryListener {
  window(telemetry: MeterTelemetry): void;
  /** The engine or runtime ended the stream; subscribe again to resume. */
  ended(): void;
}

/** Undefined state invalidates cached observations. The incarnation names the last
 * observed engine; only a fresh snapshot can confirm a pending graph revision. */
export type StateListener = (
  state: ControlStateSnapshot | undefined,
  engineIncarnation?: string,
) => void;

/** Meter readings and pushed engine state. */
export interface TelemetryPort {
  getMeters(): Promise<MeterSnapshot>;
  /** Push alternative to polling `getMeters`; resolves to an unsubscribe function. */
  subscribeMeters?(listener: MeterTelemetryListener): Promise<() => void>;
  subscribeState?(listener: StateListener): () => void;
}
