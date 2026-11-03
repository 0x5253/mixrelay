import type { PropsWithChildren } from 'react';
import { Dialog, DialogTrigger, Popover } from 'react-aria-components';
import styles from '../theme/ComponentTheme.module.css';
import { ActionButton } from './ActionButton';

export function InfoPopover({
  children,
  label,
  critical = false,
}: PropsWithChildren<{ label: string; critical?: boolean }>) {
  return (
    <DialogTrigger>
      <ActionButton tone={critical ? 'critical' : 'neutral'} density="compact">
        {label}
      </ActionButton>
      <Popover
        style={{ zIndex: 'var(--layer-floating)' }}
        className={styles.popover}
        placement="top end"
        offset={4}
      >
        <Dialog aria-label={label} className={styles.infoDialog}>
          {children}
        </Dialog>
      </Popover>
    </DialogTrigger>
  );
}
