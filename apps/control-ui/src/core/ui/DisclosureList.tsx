import type { ReactNode } from 'react';
import {
  Button,
  Disclosure,
  DisclosureGroup,
  DisclosurePanel,
  Heading,
} from 'react-aria-components';
import type { NoAppearanceOverrides } from './ActionButton';
import styles from './DisclosureList.module.css';

export interface DisclosureListItem {
  id: string;
  title: string;
  content: ReactNode;
}

/** Collapsed rows of related detail, each opening in place (Carbon's accordion). One row per
 * item keeps long reports scannable; the panel aligns with the title, not the chevron. */
export function DisclosureList({
  items,
}: NoAppearanceOverrides & { items: readonly DisclosureListItem[] }) {
  return (
    <DisclosureGroup allowsMultipleExpanded className={styles.root}>
      {items.map((item) => (
        <Disclosure key={item.id} id={item.id} className={styles.item}>
          <Heading level={3} className={styles.heading}>
            <Button slot="trigger" className={styles.trigger}>
              <span aria-hidden="true" className={styles.chevron}>
                ›
              </span>
              <span className={styles.title}>{item.title}</span>
            </Button>
          </Heading>
          <DisclosurePanel className={styles.panel}>{item.content}</DisclosurePanel>
        </Disclosure>
      ))}
    </DisclosureGroup>
  );
}
