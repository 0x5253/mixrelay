import type { ComponentPropsWithoutRef } from 'react';
import { Button, type ButtonProps } from 'react-aria-components';
import styles from '../theme/ComponentTheme.module.css';

/** `never` also rejects overrides hidden in a typed object spread. */
export interface NoAppearanceOverrides {
  className?: never;
  classNames?: never;
  style?: never;
  styles?: never;
  vars?: never;
  unstyled?: never;
  color?: never;
  c?: never;
  size?: never;
  radius?: never;
  variant?: never;
  component?: never;
  renderRoot?: never;
  dangerouslySetInnerHTML?: never;
}

export type CommandProps = Omit<ComponentPropsWithoutRef<'button'>, keyof NoAppearanceOverrides> &
  NoAppearanceOverrides;

export type ActionButtonProps = Omit<
  CommandProps,
  'onFocus' | 'onBlur' | 'onKeyDown' | 'onKeyUp' | 'onClick' | 'value' | 'children'
> &
  Pick<
    ButtonProps,
    'onFocus' | 'onBlur' | 'onKeyDown' | 'onKeyUp' | 'onClick' | 'value' | 'children'
  > & {
    tone?: 'neutral' | 'critical';
    density?: 'regular' | 'compact';
    emphasis?: 'quiet' | 'outlined' | 'strong';
    stretch?: boolean;
    align?: 'center' | 'start';
    loading?: boolean;
  };

/** Consumers choose a Core contract; visual styling stays inside Core. */
export function ActionButton({
  tone = 'neutral',
  density = 'regular',
  emphasis = 'quiet',
  stretch = false,
  align = 'center',
  loading,
  disabled,
  ...props
}: ActionButtonProps) {
  return (
    <Button
      {...props}
      render={(domProps) => (
        <button
          {...domProps}
          aria-busy={loading || props['aria-busy']}
          aria-description={props['aria-description']}
          aria-keyshortcuts={props['aria-keyshortcuts']}
        />
      )}
      isDisabled={disabled || loading}
      aria-busy={loading || props['aria-busy']}
      data-tone={tone}
      data-stretch={stretch}
      data-density={density}
      data-emphasis={emphasis}
      data-align={align}
      className={styles.command}
    />
  );
}
