import { useMidiControl } from '../midi/useMidiControl';
import { ActionButton } from './ActionButton';
import styles from './NodeSignalControls.module.css';
import { Tooltip } from './Tooltip';

export function NodePowerButton(props: {
  enabled: boolean;
  disabled: boolean;
  label: string;
  onChange: () => void;
}) {
  const mapping = useMidiControl('power');
  if (mapping?.active)
    return (
      <ActionButton
        density="compact"
        data-midi-map
        aria-label={mapping.label}
        aria-pressed={mapping.selected}
        onClick={mapping.select}
        onKeyDown={mapping.keyDown}
      >
        {mapping.text || (
          <svg aria-hidden="true" viewBox="0 0 20 20">
            <path d="M10 2v8M5 4.5a7 7 0 1 0 10 0" />
          </svg>
        )}
      </ActionButton>
    );
  return (
    <Tooltip label={props.label}>
      <button
        type="button"
        className={`${styles.button} nodrag nopan`}
        aria-label={props.label}
        aria-pressed={props.enabled}
        data-midi-selected={mapping?.selected || undefined}
        data-action="power"
        disabled={props.disabled}
        onClick={(event) => {
          event.stopPropagation();
          props.onChange();
        }}
      >
        <svg aria-hidden="true" viewBox="0 0 20 20">
          <path d="M10 2v8M5 4.5a7 7 0 1 0 10 0" />
        </svg>
      </button>
    </Tooltip>
  );
}

export function NodeMuteButton(props: {
  muted: boolean;
  disabled: boolean;
  label: string;
  onChange: () => void;
}) {
  const mapping = useMidiControl('mute');
  if (mapping?.active)
    return (
      <ActionButton
        density="compact"
        data-midi-map
        aria-label={mapping.label}
        aria-pressed={mapping.selected}
        onClick={mapping.select}
        onKeyDown={mapping.keyDown}
      >
        {mapping.text || (
          <svg aria-hidden="true" viewBox="0 0 20 20">
            <path d="M3 7h3l4-4v14l-4-4H3z" />
          </svg>
        )}
      </ActionButton>
    );
  return (
    <Tooltip label={props.label}>
      <button
        type="button"
        className={`${styles.button} nodrag nopan`}
        aria-label={props.label}
        aria-pressed={props.muted}
        data-midi-selected={mapping?.selected || undefined}
        data-action="mute"
        disabled={props.disabled}
        onClick={(event) => {
          event.stopPropagation();
          props.onChange();
        }}
      >
        <svg aria-hidden="true" viewBox="0 0 20 20">
          <path d="M3 7h3l4-4v14l-4-4H3z" />
          {props.muted ? (
            <path d="m13 7 5 6m0-6-5 6" />
          ) : (
            <path d="M13 6a6 6 0 0 1 0 8m2-11a10 10 0 0 1 0 14" />
          )}
        </svg>
      </button>
    </Tooltip>
  );
}
