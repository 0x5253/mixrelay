import type { ReactNode } from 'react';
import type { NoAppearanceOverrides } from './ActionButton';
import styles from './InlineNotice.module.css';
import type { StatusTone } from './StatusBadge';

/** Local feedback stays with the affected operation; critical changes are announced urgently. */
export function InlineNotice({
  children,
  title,
  tone = 'neutral',
  actions,
  announcement,
  inert,
}: NoAppearanceOverrides & {
  children: ReactNode;
  title?: string;
  tone?: StatusTone;
  actions?: ReactNode;
  announcement?: 'status' | 'alert';
  inert?: boolean;
}) {
  return (
    <div
      className={styles.root}
      data-tone={tone}
      role={announcement ?? (tone === 'critical' ? 'alert' : 'status')}
      inert={inert}
    >
      <div className={styles.content}>
        {title ? <strong className={styles.title}>{title}</strong> : null}
        <div>{children}</div>
      </div>
      {actions ? <div className={styles.actions}>{actions}</div> : null}
    </div>
  );
}
