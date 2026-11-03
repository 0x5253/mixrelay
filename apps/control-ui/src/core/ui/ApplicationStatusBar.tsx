import type { ReactNode } from 'react';
import styles from './ApplicationStatusBar.module.css';

/** App information, audio transport and diagnostic access share a fixed, single row.
 * `announcement` politely reports a status change that the row shows only as a count. */
export function ApplicationStatusBar({
  information,
  controls,
  announcement = '',
}: {
  information?: ReactNode;
  controls: ReactNode;
  announcement?: string;
}) {
  return (
    <footer className={styles.root}>
      <div className={styles.information}>{information}</div>
      <div className={styles.controls}>{controls}</div>
      <span role="status" className={styles.announcement}>
        {announcement}
      </span>
    </footer>
  );
}
