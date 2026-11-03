import { type ComponentProps, type ReactElement, useState } from 'react';
import { Tooltip as AriaTooltip, Focusable, TooltipTrigger } from 'react-aria-components';
import styles from '../theme/ComponentTheme.module.css';
import { motion } from '../theme/tokens';

/** Non-interactive markers never become an extra Tab stop; their child must be a named
 * graphic (`role="img"` with `aria-label`). `readout` is the compact monospaced value form
 * used next to graph ports. */
export function Tooltip({
  children,
  label,
  position = 'top',
  delay = motion.tooltipDelay,
  forceOpen = false,
  onClose,
  kind = 'label',
  offset = 4,
  nonInteractive = false,
}: {
  children: ReactElement;
  label: string;
  position?: 'top' | 'bottom' | 'left' | 'right';
  delay?: number;
  /** Keeps the tooltip open while true; otherwise hover and focus open it as usual. */
  forceOpen?: boolean;
  /** Called when the tooltip asks to close, including Escape on a forced tooltip. */
  onClose?: () => void;
  kind?: 'label' | 'readout';
  offset?: number;
  nonInteractive?: boolean;
}) {
  const nativeProps = children.props as { disabled?: boolean };
  // The trigger stays controlled for its whole life: hover/focus state lives here, so a
  // forced open never switches React Aria between controlled and uncontrolled.
  const [requested, setRequested] = useState(false);
  return (
    <TooltipTrigger
      delay={delay}
      closeDelay={0}
      isOpen={forceOpen || requested}
      onOpenChange={(open) => {
        setRequested(open);
        if (!open) onClose?.();
      }}
    >
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
