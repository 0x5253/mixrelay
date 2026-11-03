import type { ComponentPropsWithoutRef, ReactNode } from 'react';
import type { NoAppearanceOverrides } from './ActionButton';
import styles from './StateButton.module.css';
import type { StatusTone } from './StatusBadge';

export interface ButtonState {
  value: string;
  label: string;
  icon: ReactNode;
}

/** Reserves every translated state in one grid cell, keeping the click target stable. */
export function StateButton({
  states,
  value,
  tone = 'neutral',
  ...props
}: Omit<ComponentPropsWithoutRef<'button'>, 'children' | 'value' | keyof NoAppearanceOverrides> &
  NoAppearanceOverrides & {
    states: readonly ButtonState[];
    value: string;
    tone?: StatusTone;
  }) {
  const selected = states.find((state) => state.value === value);
  return (
    <button
      type="button"
      {...props}
      className={styles.button}
      data-tone={tone}
      aria-label={props['aria-label'] ?? selected?.label}
    >
      {states.map((state) => (
        <span
          key={state.value}
          className={styles.state}
          data-visible={state.value === value}
          aria-hidden="true"
        >
          <span className={styles.icon}>{state.icon}</span>
          <span>{state.label}</span>
        </span>
      ))}
    </button>
  );
}
