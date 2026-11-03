import type { PropsWithChildren } from 'react';
import type { NoAppearanceOverrides } from './ActionButton';
import styles from './Layout.module.css';
import type { LayoutGap } from './Stack';

export function Inline({
  children,
  gap = 'regular',
  align = 'center',
  justify = 'start',
  wrap = true,
}: PropsWithChildren<
  NoAppearanceOverrides & {
    gap?: LayoutGap;
    align?: 'start' | 'center' | 'baseline';
    justify?: 'start' | 'center' | 'end' | 'between';
    wrap?: boolean;
  }
>) {
  return (
    <div
      className={styles.inline}
      data-gap={gap}
      data-align={align}
      data-justify={justify}
      data-wrap={wrap}
    >
      {children}
    </div>
  );
}
