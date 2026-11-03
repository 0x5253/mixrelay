import type { StatusTone } from './StatusBadge';
import styles from './StatusIndicator.module.css';
import { Tooltip } from './Tooltip';

/** The tooltip and accessible name describe exactly the same status as the colored marker. */
export function StatusIndicator({
  label,
  tone = 'neutral',
  position = 'top',
}: {
  label: string;
  tone?: StatusTone;
  position?: 'top' | 'bottom' | 'left' | 'right';
}) {
  return (
    <Tooltip nonInteractive label={label} position={position}>
      <span className={styles.root} data-tone={tone} role="img" aria-label={label} />
    </Tooltip>
  );
}
