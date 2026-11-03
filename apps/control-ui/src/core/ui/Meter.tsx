import type { CSSProperties } from 'react';

import styles from './Meter.module.css';

interface MeterProps {
  orientation?: 'horizontal' | 'vertical';
  label: string;
  level: number;
}

export function Meter({ label, level, orientation = 'vertical' }: MeterProps) {
  const normalizedLevel = Math.max(0, Math.min(100, level));
  return (
    // biome-ignore lint/a11y/useSemanticElements: Custom audio level painting with read-only meter semantics.
    <span
      className={styles.root}
      data-orientation={orientation}
      role="meter"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={normalizedLevel}
      title={`${label}: ${normalizedLevel}%`}
    >
      <span
        className={styles.fill}
        style={{ '--meter-unfilled': `${100 - normalizedLevel}%` } as CSSProperties}
      />
    </span>
  );
}
