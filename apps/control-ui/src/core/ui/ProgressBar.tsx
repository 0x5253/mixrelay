import type { CSSProperties } from 'react';
import { ProgressBar as AriaProgressBar, Label } from 'react-aria-components';
import styles from './ProgressBar.module.css';

/**
 * Determinate progress: what is happening now, how far it is, and a bar that shows the same.
 * The label names the current work; the value text (for example "Step 2 of 3") is shown and
 * exposed as the progress bar's value. A progressbar is not announced when it changes, so the
 * label is also announced once per change through a polite status.
 */
export function ProgressBar({
  label,
  value,
  maxValue,
  valueLabel,
}: {
  label: string;
  value: number;
  maxValue: number;
  valueLabel: string;
}) {
  // Always determinate, so the fill follows the given value rather than React Aria's optional one.
  const filled = Math.min(100, Math.max(0, (value / maxValue) * 100));
  return (
    <AriaProgressBar
      className={styles.root}
      value={value}
      minValue={0}
      maxValue={maxValue}
      valueLabel={valueLabel}
    >
      {({ valueText }) => (
        <>
          <span className={styles.header}>
            <Label className={styles.label}>{label}</Label>
            <span className={styles.value}>{valueText}</span>
          </span>
          <span className={styles.announcement} role="status">
            {label}
          </span>
          <span className={styles.track}>
            <span
              className={styles.fill}
              style={{ '--progress-filled': `${filled}%` } as CSSProperties}
            />
          </span>
        </>
      )}
    </AriaProgressBar>
  );
}
