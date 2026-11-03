import { type PropsWithChildren, useId } from 'react';
import type { NoAppearanceOverrides } from './ActionButton';
import styles from './ContentSection.module.css';
import { Stack } from './Stack';

/** Named content groups share a heading, rhythm and optional separator. */
export function ContentSection({
  children,
  title,
  level = 2,
  divider = 'none',
}: PropsWithChildren<
  NoAppearanceOverrides & { title: string; level?: 2 | 3; divider?: 'none' | 'before' | 'after' }
>) {
  const titleId = useId();
  const Heading = level === 2 ? 'h2' : 'h3';
  return (
    <section
      className={styles.root}
      data-divider={divider}
      data-level={level}
      aria-labelledby={titleId}
    >
      <Stack>
        <Heading id={titleId} className={styles.title}>
          {title}
        </Heading>
        {children}
      </Stack>
    </section>
  );
}
