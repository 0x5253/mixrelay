import type { ReactNode, Ref } from 'react';
import styles from './AudioNodeCard.module.css';
import type { AudioNodeParameterControl } from './AudioNodeParameterGrid';
import { bypassControl, bypassLabel } from './audioNodeCardModel';
import { IconActionButton } from './IconActionButton';
import { useNodeHeaderFocus } from './useNodeHeaderFocus';

/** The header's action slots: power, mute, a built-in bypass and the Options trigger. */
export function AudioNodeHeaderActions({
  title,
  actions,
  mute,
  bypassControls,
  bypassDisabled,
  onBypassChange,
  headerRef,
  targetRef,
}: {
  title: string;
  actions?: ReactNode;
  mute?: ReactNode;
  bypassControls: readonly AudioNodeParameterControl[];
  bypassDisabled: boolean;
  onBypassChange?: (parameterId: string, value: number) => void;
  headerRef: Ref<HTMLDivElement>;
  /** Where the Options disclosure portals its trigger. */
  targetRef: Ref<HTMLDivElement>;
}) {
  const focus = useNodeHeaderFocus();
  return (
    <div className={styles.headerActions} ref={headerRef} {...focus}>
      <div className={styles.headerSlot}>{actions}</div>
      <div className={styles.headerSlot}>{mute}</div>
      <div className={styles.headerSlot}>
        <BuiltinBypass
          title={title}
          controls={bypassControls}
          disabled={bypassDisabled}
          onChange={onBypassChange}
        />
      </div>
      <div className={styles.headerSlot} ref={targetRef} />
    </div>
  );
}

function BuiltinBypass({
  title,
  controls,
  disabled,
  onChange,
}: {
  title: string;
  controls: readonly AudioNodeParameterControl[];
  disabled: boolean;
  onChange?: (parameterId: string, value: number) => void;
}) {
  const control = bypassControl(controls);
  if (!control || !onChange) return null;
  return (
    <IconActionButton
      label={bypassLabel(title, control)}
      aria-pressed={Boolean(control.value)}
      disabled={disabled}
      onClick={() => onChange(control.id, control.value ? 0 : 1)}
    >
      <svg viewBox="0 0 20 20" aria-hidden="true">
        <path d="M2 10h4m8 0h4M6 6h8v8H6z" />
        {control.value ? <path d="m4 16 12-12" /> : null}
      </svg>
    </IconActionButton>
  );
}
