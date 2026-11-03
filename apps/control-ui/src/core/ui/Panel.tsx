import type { PropsWithChildren, ReactNode } from 'react';

import styles from './Panel.module.css';

interface PanelProps extends PropsWithChildren {
  title: string;
  trailing?: ReactNode;
}

export function Panel({ children, title, trailing }: PanelProps) {
  return (
    <section className={styles.root}>
      <div className={styles.header}>
        <h2 className={styles.title}>{title}</h2>
        {trailing}
      </div>
      {children}
    </section>
  );
}
