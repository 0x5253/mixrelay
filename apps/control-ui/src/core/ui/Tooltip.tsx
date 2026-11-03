import type { ComponentProps, ReactElement } from 'react';
import { Tooltip as AriaTooltip, Focusable, TooltipTrigger } from 'react-aria-components';
import styles from '../theme/ComponentTheme.module.css';
import { motion } from '../theme/tokens';

/** Non-interactive markers never become an extra Tab stop. `readout` is the compact
 * monospaced value form used next to graph ports. */
export function Tooltip({
  children,
  label,
  position = 'top',
  delay = motion.tooltipDelay,
  opened,
  onOpenChange,
  kind = 'label',
  offset = 4,
  nonInteractive = false,
}: {
  children: ReactElement;
  label: string;
  position?: 'top' | 'bottom' | 'left' | 'right';
  delay?: number;
  opened?: boolean;
  /** With `opened`, lets the tooltip ask to close (Escape) instead of staying forced open. */
  onOpenChange?: (open: boolean) => void;
  kind?: 'label' | 'readout';
  offset?: number;
  nonInteractive?: boolean;
}) {
  const nativeProps = children.props as { disabled?: boolean };
  return (
    <TooltipTrigger delay={delay} closeDelay={0} isOpen={opened} onOpenChange={onOpenChange}>
      {typeof children.type === 'string' || nonInteractive ? (
        <Focusable isDisabled={nativeProps.disabled} excludeFromTabOrder={nonInteractive}>
          {children as ComponentProps<typeof Focusable>['children']}
        </Focusable>
      ) : (
        children
      )}
      <AriaTooltip
        style={{ zIndex: 'var(--layer-tooltip)' }}
        className={
          kind === 'readout' ? `${styles.tooltip} ${styles.tooltipReadout}` : styles.tooltip
        }
        placement={position}
        offset={offset}
      >
        {label}
      </AriaTooltip>
    </TooltipTrigger>
  );
}
