import styles from './AudioNodeCard.module.css';

/** Sampled engine/worker round trip; avoid precision finer than the recorded clock unit. */
export function ProcessorTimingLabel({ nanoseconds }: { nanoseconds?: number }) {
  if (nanoseconds === undefined || !Number.isFinite(nanoseconds) || nanoseconds <= 0) return null;
  const [value, unit] =
    nanoseconds >= 1_000_000
      ? [nanoseconds / 1_000_000, 'ms']
      : nanoseconds >= 1_000
        ? [nanoseconds / 1_000, 'µs']
        : [nanoseconds, 'ns'];
  return (
    <span className={styles.processingTime}>
      {value.toFixed(unit === 'ns' ? 0 : 2)} {unit}
    </span>
  );
}
