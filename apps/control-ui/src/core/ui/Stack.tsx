import type { PropsWithChildren } from 'react';
import type { NoAppearanceOverrides } from './ActionButton';
import styles from './Layout.module.css';

export type LayoutGap = 'tight' | 'regular' | 'section' | 'wide';

/** Spacing belongs to the composition; children do not add their own margins. */
export function Stack({
  children,
  gap = 'regular',
  align = 'stretch',
}: PropsWithChildren<NoAppearanceOverrides & { gap?: LayoutGap; align?: 'stretch' | 'center' }>) {
  return (
    <div className={styles.stack} data-gap={gap} data-align={align}>
      {children}
    </div>
  );
}
