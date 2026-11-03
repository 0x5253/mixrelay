import type { ReactNode } from 'react';
import type { CommandProps } from './ActionButton';
import styles from './IconActionButton.module.css';
import { Tooltip } from './Tooltip';

export function IconActionButton({
  label,
  children,
  tone = 'neutral',
  density = 'compact',
  indicator = false,
  ...props
}: CommandProps & {
  label: string;
  children: ReactNode;
  tone?: 'neutral' | 'critical' | 'warning';
  density?: 'regular' | 'compact';
  indicator?: boolean;
}) {
  return (
    <Tooltip label={label}>
      <button
        type="button"
        {...props}
        className={`${styles.root} nodrag nopan`}
        aria-label={label}
        data-tone={tone}
        data-density={density}
      >
        {children}
        {indicator ? <span className={styles.indicator} aria-hidden="true" /> : null}
      </button>
    </Tooltip>
  );
}
