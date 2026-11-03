import type { MeterSnapshot } from '../domain/control/meters';
import { meterScale } from '../theme/tokens';

const EMPTY_LEVELS: readonly number[] = [];
type Listener = () => void;

export interface DynamicsMeterReading {
  inputPeaks: readonly number[];
  outputPeaks: readonly number[];
  gainReductionDb: number;
  thresholdDb: number;
}

export class AudioMeterStore {
  readonly #levels = new Map<string, readonly number[]>();
  readonly #listeners = new Map<string, Set<Listener>>();
  readonly #dynamics = new Map<string, DynamicsMeterReading>();

  /** Invalid telemetry must not keep displaying the last positive signal. */
  public clear(): void {
    const keys = new Set([...this.#levels.keys(), ...this.#dynamics.keys()]);
    this.#levels.clear();
    this.#dynamics.clear();
    for (const key of keys) this.#notify(key);
  }

  public update(snapshot: MeterSnapshot): void {
    const present = new Set<string>();
    const changed = new Set<string>();
    for (const node of snapshot.nodes) {
      const key = audioMeterKey(node.nodeId, node.portId);
      const levels = node.peaks.map(peakToPercent);
      present.add(key);
      if (!sameLevels(this.#levels.get(key), levels)) {
        this.#levels.set(key, levels);
        changed.add(key);
      }
      const reading = dynamicsReading(node);
      if (!sameDynamics(this.#dynamics.get(key), reading)) {
        if (reading) this.#dynamics.set(key, reading);
        else this.#dynamics.delete(key);
        changed.add(key);
      }
    }
    for (const key of this.#levels.keys()) {
      if (!present.has(key)) {
        this.#levels.delete(key);
        this.#dynamics.delete(key);
        changed.add(key);
      }
    }
    for (const key of changed) this.#notify(key);
  }

  public dynamics(key: string): DynamicsMeterReading | null {
    return this.#dynamics.get(key) ?? null;
  }

  public levels(key: string): readonly number[] {
    return this.#levels.get(key) ?? EMPTY_LEVELS;
  }

  public subscribe(key: string, listener: Listener): () => void {
    const listeners = this.#listeners.get(key) ?? new Set<Listener>();
    listeners.add(listener);
    this.#listeners.set(key, listeners);
    return () => {
      listeners.delete(listener);
      if (listeners.size === 0) this.#listeners.delete(key);
    };
  }

  #notify(key: string): void {
    for (const listener of this.#listeners.get(key) ?? []) listener();
  }
}

function dynamicsReading(node: MeterSnapshot['nodes'][number]): DynamicsMeterReading | undefined {
  const value = node.dynamics;
  if (
    !value ||
    !Number.isFinite(value.gainReductionDb) ||
    value.gainReductionDb < 0 ||
    !Number.isFinite(value.thresholdDb) ||
    value.inputPeaks.length === 0 ||
    node.peaks.length === 0
  )
    return undefined;
  const finitePeak = (peak: number) => (Number.isFinite(peak) && peak > 0 ? peak : 0);
  return {
    inputPeaks: value.inputPeaks.map(finitePeak),
    outputPeaks: node.peaks.map(finitePeak),
    gainReductionDb: value.gainReductionDb,
    thresholdDb: value.thresholdDb,
  };
}

function sameDynamics(
  current: DynamicsMeterReading | undefined,
  next: DynamicsMeterReading | undefined,
) {
  if (!current || !next) return current === next;
  const samePeaks = (left: readonly number[], right: readonly number[]) =>
    left.length === right.length && left.every((peak, index) => peak === right[index]);
  return (
    current.gainReductionDb === next.gainReductionDb &&
    current.thresholdDb === next.thresholdDb &&
    samePeaks(current.inputPeaks, next.inputPeaks) &&
    samePeaks(current.outputPeaks, next.outputPeaks)
  );
}

export function audioMeterKey(nodeId: string, portId: string): string {
  return `${encodeURIComponent(nodeId)}:${encodeURIComponent(portId)}`;
}

export function emptyAudioMeterLevels(): readonly number[] {
  return EMPTY_LEVELS;
}

function peakToPercent(peak: number): number {
  if (!Number.isFinite(peak) || peak <= 0) return 0;
  const decibels = 20 * Math.log10(peak);
  const floor = meterScale.floorDb;
  return Math.max(0, Math.min(100, ((decibels - floor) / -floor) * 100));
}

function sameLevels(current: readonly number[] | undefined, next: readonly number[]): boolean {
  return (
    current?.length === next.length &&
    // Native peak arrays and their mapped levels are dense. Equal lengths guarantee
    // the previous slot; absence of a channel is handled by the length comparison.
    next.every((level, index) => Math.abs(level - (current[index] as number)) < 0.1)
  );
}
