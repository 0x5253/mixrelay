import type { PropsWithChildren } from 'react';
import styles from './StatusBadge.module.css';

export type StatusTone = 'neutral' | 'accent' | 'positive' | 'warning' | 'critical';

interface StatusBadgeProps extends PropsWithChildren {
  tone?: StatusTone;
}

export function StatusBadge({ children, tone = 'neutral' }: StatusBadgeProps) {
  return (
    <span className={styles.root} data-tone={tone}>
      {children}
    </span>
  );
}
