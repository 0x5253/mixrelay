import type { PropsWithChildren } from 'react';
import type { NoAppearanceOverrides } from './ActionButton';
import type { StatusTone } from './StatusBadge';
import styles from './Text.module.css';

/** Typography roles come from the theme; composition owns the surrounding spacing. */
export function Text({
  children,
  kind = 'body',
  tone = 'neutral',
  as: Element = 'p',
  id,
}: PropsWithChildren<
  NoAppearanceOverrides & {
    kind?: 'heading' | 'body' | 'supporting' | 'label' | 'caption';
    tone?: StatusTone | 'muted';
    as?: 'h1' | 'h2' | 'p' | 'span' | 'small';
    /** For aria-labelledby relationships; the element still owns no spacing. */
    id?: string;
  }
>) {
  return (
    <Element className={styles.root} data-text-role={kind} data-tone={tone} id={id}>
      {children}
    </Element>
  );
}
