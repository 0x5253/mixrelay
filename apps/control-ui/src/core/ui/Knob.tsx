import { type KeyboardEvent, useRef } from 'react';
import { knobDragPosition } from '../domain/parameters/parameterGesture';
import {
  parameterAtPosition,
  parameterPosition,
  snapParameter,
  steppedParameterKey,
} from '../domain/parameters/parameterRange';
import styles from './Knob.module.css';
import { useCapturedPointerDrag } from './useCapturedPointerDrag';

export interface KnobProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  scale?: 'linear' | 'logarithmic';
  disabled?: boolean;
  valueText?: string;
  endpoints?: { minimum: string; maximum: string };
  onChange: (value: number) => void;
}

function useKnobInteraction({
  value,
  min,
  max,
  step = 1,
  scale = 'linear',
  disabled,
  onChange,
}: KnobProps) {
  const input = useRef<HTMLInputElement>(null);
  const range = { minimum: min, maximum: max, step, scale };
  const normalize = (next: number) => snapParameter(next, range);
  const pointer = useCapturedPointerDrag({
    disabled,
    begin(event) {
      event.preventDefault();
      input.current?.focus();
      return { y: event.clientY, position: parameterPosition(value, range) };
    },
    update(active, event) {
      // The continuous position accumulates; only reported values snap to the step.
      active.position = knobDragPosition(
        active.position,
        active.y - event.clientY,
        event.shiftKey,
        range,
      );
      active.y = event.clientY;
      onChange(normalize(parameterAtPosition(active.position, range)));
    },
  });
  const key = (event: KeyboardEvent<HTMLInputElement>) => {
    if (disabled) return;
    const next = steppedParameterKey(event.key, value, range);
    if (next === undefined) return;
    event.preventDefault();
    event.stopPropagation();
    onChange(normalize(next));
  };
  return {
    input,
    fraction: parameterPosition(value, range),
    origin: min < 0 && max > 0 ? parameterPosition(0, range) : 0,
    pointer,
    key,
    normalize,
  };
}

/** A vector dial over a native range input. Drag up to increase; Shift makes fine adjustments. */
export function Knob(props: KnobProps) {
  const { label, value, min, max, disabled, valueText, endpoints, onChange } = props;
  const { input, fraction, origin, pointer, key, normalize } = useKnobInteraction(props);
  const bipolar = min < 0 && max > 0;
  const angle = -135 + fraction * 270;
  return (
    <div
      className={`${styles.root} nodrag nopan nowheel nokey`}
      data-disabled={disabled || undefined}
      {...pointer}
    >
      <svg className={styles.dial} viewBox="0 0 48 48" aria-hidden="true">
        <path className={styles.focusArc} d="M7.737 40.263 A23 23 0 1 1 40.263 40.263" />
        <path
          className={styles.track}
          d="M 9.858 38.142 A 20 20 0 1 1 38.142 38.142"
          pathLength="1"
        />
        <path
          className={styles.value}
          d="M 9.858 38.142 A 20 20 0 1 1 38.142 38.142"
          pathLength="1"
          strokeDasharray={`${Math.abs(fraction - origin)} 1`}
          strokeDashoffset={-Math.min(fraction, origin)}
          visibility={fraction === origin ? 'hidden' : undefined}
        />
        {bipolar ? (
          <line
            className={styles.center}
            x1="24"
            y1="8"
            x2="24"
            y2="10"
            transform={`rotate(${-135 + origin * 270} 24 24)`}
          />
        ) : null}
        <line
          className={styles.indicator}
          x1="24"
          y1="5"
          x2="24"
          y2="21"
          transform={`rotate(${angle} 24 24)`}
        />
      </svg>
      {endpoints ? (
        <span className={styles.endpoints} aria-hidden="true">
          <span>{endpoints.minimum}</span>
          <span>{endpoints.maximum}</span>
        </span>
      ) : null}
      <input
        ref={input}
        className={styles.input}
        type="range"
        aria-label={label}
        aria-valuetext={valueText}
        aria-valuenow={value}
        min={min}
        max={max}
        step="any"
        value={value}
        disabled={disabled}
        onKeyDown={key}
        onChange={(event) => onChange(normalize(Number(event.target.value)))}
      />
    </div>
  );
}
