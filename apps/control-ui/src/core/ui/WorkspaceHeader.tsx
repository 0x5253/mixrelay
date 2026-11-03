import type { PropsWithChildren } from 'react';
import styles from './WorkspaceHeader.module.css';

/** One sizing rule for adjacent workspace headers, including their bottom border. */
export function WorkspaceHeader({ children }: PropsWithChildren) {
  return <header className={styles.root}>{children}</header>;
}
