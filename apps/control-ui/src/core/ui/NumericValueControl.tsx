import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { Input, NumberField } from 'react-aria-components';
import { numericDragValue, wheelSteps } from '../domain/parameters/parameterGesture';
import {
  type ParameterRange,
  snapParameter,
  steppedParameterKey,
} from '../domain/parameters/parameterRange';
import styles from './NumericValueControl.module.css';
import { useCapturedPointerDrag } from './useCapturedPointerDrag';

export interface NumericValueControlProps {
  label: string;
  value: number;
  minimum: number;
  maximum: number;
  step: number;
  scale?: 'linear' | 'logarithmic';
  suffix?: string;
  density?: 'regular' | 'compact';
  disabled?: boolean;
  onChange: (value: number) => void;
  onActivate?: () => void;
}
function rangeOf(props: NumericValueControlProps): ParameterRange {
  return {
    minimum: props.minimum,
    maximum: props.maximum,
    step: props.step,
    scale: props.scale,
  };
}
function normalize(value: number, props: NumericValueControlProps) {
  return snapParameter(value, rangeOf(props));
}
function useValueDrag(props: NumericValueControlProps) {
  return useCapturedPointerDrag({
    disabled: props.disabled,
    begin(event) {
      event.currentTarget.querySelector('input')?.focus();
      return { y: event.clientY, value: props.value };
    },
    update(active, event) {
      if (active.y === event.clientY) return;
      event.preventDefault();
      event.stopPropagation();
      // Only the reported value snaps; the dragged value stays continuous.
      active.value = numericDragValue(
        active.value,
        active.y - event.clientY,
        event.shiftKey,
        rangeOf(props),
      );
      active.y = event.clientY;
      props.onChange(normalize(active.value, props));
    },
  });
}
function useValueWheel(props: NumericValueControlProps) {
  const ref = useRef<HTMLDivElement>(null);
  const current = useRef(props);
  current.current = props;
  const remainder = useRef(0);
  const pending = useRef(props.value);
  const observed = useRef(props.value);
  if (observed.current !== props.value) {
    pending.current = props.value;
    observed.current = props.value;
    remainder.current = 0;
  }
  useEffect(() => {
    const element = ref.current as HTMLDivElement;
    const wheel = (event: WheelEvent) => {
      event.preventDefault();
      event.stopPropagation();
      const options = current.current;
      if (options.disabled || event.deltaY === 0) return;
      options.onActivate?.();
      const turned = wheelSteps(remainder.current, event.deltaY, event.shiftKey);
      remainder.current = turned.remainder;
      if (!turned.steps) return;
      pending.current = normalize(pending.current + turned.steps * options.step, options);
      options.onChange(pending.current);
    };
    element.addEventListener('wheel', wheel, { passive: false });
    return () => element.removeEventListener('wheel', wheel);
  }, []);
  return ref;
}

/** One precise value owner with direct entry, unit-step keys/wheel and continuous drag. */
export function NumericValueControl(props: NumericValueControlProps) {
  const unitId = useId();
  const drag = useValueDrag(props);
  const ref = useValueWheel(props);
  const text = String(Number(props.value.toFixed(3)));
  const [draft, setDraft] = useState<string | null>(null);
  useLayoutEffect(() => {
    // A committed wheel/key/graph change supersedes the local editing width.
    void props.value;
    setDraft(null);
  }, [props.value]);
  return (
    <div
      ref={ref}
      className={`${styles.root} nodrag nopan nowheel nokey`}
      data-density={props.density}
      {...drag}
      onKeyDownCapture={(event) => {
        if (props.disabled) return;
        const next = steppedParameterKey(event.key, props.value, rangeOf(props), 'vertical');
        if (next === undefined) return;
        event.preventDefault();
        event.stopPropagation();
        props.onChange(normalize(next, props));
      }}
    >
      <NumberField
        aria-label={props.label}
        value={props.value}
        minValue={props.minimum}
        maxValue={props.maximum}
        step={props.step}
        isDisabled={props.disabled}
        formatOptions={{ maximumFractionDigits: 3, useGrouping: false }}
        onChange={(value) => {
          if (Number.isFinite(value)) props.onChange(value);
        }}
      >
        <div className={styles.bundle}>
          <Input
            className={styles.input}
            style={{ width: `${Math.max(1, (draft ?? text).length)}ch` }}
            aria-describedby={props.suffix ? unitId : undefined}
            onFocus={(event) => {
              setDraft(event.currentTarget.value);
              props.onActivate?.();
            }}
            onChange={(event) => setDraft(event.currentTarget.value)}
            onBlur={() => setDraft(null)}
          />
          {props.suffix ? (
            <span className={styles.unit} id={unitId}>
              {props.suffix}
            </span>
          ) : null}
        </div>
      </NumberField>
    </div>
  );
}
