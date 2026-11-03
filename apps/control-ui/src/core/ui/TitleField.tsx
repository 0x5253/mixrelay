import type { ComponentPropsWithoutRef, Ref } from 'react';
import type { NoAppearanceOverrides } from './ActionButton';
import styles from './TitleField.module.css';

export function TitleField(
  props: Omit<ComponentPropsWithoutRef<'input'>, keyof NoAppearanceOverrides> &
    NoAppearanceOverrides & { ref?: Ref<HTMLInputElement> },
) {
  return <input {...props} className={styles.root} />;
}
