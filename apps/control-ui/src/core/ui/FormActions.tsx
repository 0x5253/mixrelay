import type { PropsWithChildren } from 'react';
import type { NoAppearanceOverrides } from './ActionButton';
import styles from './Layout.module.css';

/** One action-row contract for forms and recovery workflows, including long translations. */
export function FormActions({
  children,
  align = 'end',
}: PropsWithChildren<NoAppearanceOverrides & { align?: 'start' | 'end' }>) {
  return (
    <div className={styles.actions} data-align={align}>
      {children}
    </div>
  );
}
