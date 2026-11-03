import { createContext, type ReactNode, useContext, useId, useLayoutEffect } from 'react';
import { Button } from 'react-aria-components';
import { createPortal } from 'react-dom';
import { MidiContext } from '../midi/MidiContext';
import styles from './AudioNodeCard.module.css';
import { Tooltip } from './Tooltip';
import { useElementBounds } from './useElementBounds';

export const NodeDisclosureMeasureContext = createContext<(() => void) | null>(null);
export const NodeHeaderOptionsContext = createContext<HTMLElement | null>(null);

/** Shared node disclosure; expanding settings never changes the cable anchor face. */
export function NodeOptionsSection({
  label,
  opened,
  onToggle,
  children,
  description,
  disabled = false,
}: {
  label: string;
  opened: boolean;
  onToggle: () => void;
  children: ReactNode;
  description?: ReactNode;
  disabled?: boolean;
}) {
  const mapping = useContext(MidiContext)?.active ?? false;
  const measureDisclosure = useContext(NodeDisclosureMeasureContext);
  const headerTarget = useContext(NodeHeaderOptionsContext);
  const panelId = useId();
  const [panelRef, panelBounds] = useElementBounds<HTMLDivElement>();
  useLayoutEffect(() => {
    // The panel's observer and open state only signal when to read both live boxes.
    void opened;
    void panelBounds.height;
    void panelBounds.width;
    measureDisclosure?.();
  }, [opened, panelBounds.height, panelBounds.width, measureDisclosure]);
  const trigger = (
    <Tooltip label={label}>
      <Button
        isDisabled={mapping}
        aria-label={label}
        aria-expanded={opened}
        aria-controls={opened ? panelId : undefined}
        className={`${styles.optionsToggle} nodrag nopan nowheel`}
        onClick={(event) => {
          event.stopPropagation();
          onToggle();
        }}
      >
        <span
          aria-hidden="true"
          className={styles.optionsChevron}
          data-opened={opened || undefined}
        >
          ›
        </span>
      </Button>
    </Tooltip>
  );
  return (
    <>
      {headerTarget ? createPortal(trigger, headerTarget) : trigger}
      {opened ? (
        <div
          ref={panelRef}
          id={panelId}
          data-node-disclosure=""
          className={`${styles.optionsPanel} nodrag nopan nowheel`}
          inert={disabled || mapping}
        >
          {description ? <small className={styles.settingsDescription}>{description}</small> : null}
          {children}
        </div>
      ) : null}
    </>
  );
}
