import type { ReactNode } from 'react';
import styles from './AudioNodeCard.module.css';

/** Node utility row: a stable value at left and compact transport actions at right. */
export function NodeTransportRow({
  value,
  label,
  state,
  description,
  children,
}: {
  value: ReactNode;
  label: string;
  state?: string;
  description?: string;
  children: ReactNode;
}) {
  // One summary serves the accessible description and the pointer tooltip; empty is omitted.
  const summary = [state, description].filter(Boolean).join(' · ') || undefined;
  return (
    <div className={styles.transportRow}>
      <output
        className={styles.transportValue}
        aria-label={label}
        aria-live="off"
        aria-description={summary}
        title={summary}
      >
        {value}
      </output>
      <div className={styles.transportActions}>{children}</div>
    </div>
  );
}
