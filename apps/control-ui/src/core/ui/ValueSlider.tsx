import { useLayoutEffect, useRef } from 'react';
import { Slider, SliderThumb, SliderTrack } from 'react-aria-components';
import {
  clampParameter,
  isLogarithmicRange,
  sliderScale,
  steppedParameterKey,
} from '../domain/parameters/parameterRange';
import styles from '../theme/ComponentTheme.module.css';

/** Core slider behavior and hit area, shared by levels and node parameters. */
export function ValueSlider({
  label,
  value,
  min,
  max,
  step = 1,
  disabled,
  onChange,
  valueText,
  scale = 'linear',
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  disabled?: boolean;
  valueText?: string;
  scale?: 'linear' | 'logarithmic';
  onChange: (value: number) => void;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const range = { minimum: min, maximum: max, step, scale };
  const logarithmic = isLogarithmicRange(range);
  const slider = sliderScale(range);
  useLayoutEffect(() => {
    if (valueText) inputRef.current?.setAttribute('aria-valuetext', valueText);
    else inputRef.current?.removeAttribute('aria-valuetext');
    if (logarithmic) {
      inputRef.current?.setAttribute('aria-valuemin', String(min));
      inputRef.current?.setAttribute('aria-valuemax', String(max));
      inputRef.current?.setAttribute('aria-valuenow', String(value));
    } else {
      for (const attribute of ['aria-valuemin', 'aria-valuemax', 'aria-valuenow'])
        inputRef.current?.removeAttribute(attribute);
    }
  }, [valueText, logarithmic, min, max, value]);
  return (
    <div
      onKeyDownCapture={(event) => {
        if (!logarithmic || disabled) return;
        const next = steppedParameterKey(event.key, value, range);
        if (next === undefined) return;
        event.preventDefault();
        event.stopPropagation();
        onChange(clampParameter(next, range));
      }}
    >
      <Slider
        className={`${styles.sliderRoot} nodrag nopan nowheel`}
        aria-label={label}
        value={slider.toSlider(value)}
        minValue={slider.minValue}
        maxValue={slider.maxValue}
        step={slider.step}
        isDisabled={disabled}
        onChange={(next) => onChange(slider.fromSlider(next))}
      >
        <SliderTrack className={styles.sliderTrack}>
          {({ state }) => (
            <>
              <span
                className={styles.sliderBar}
                style={{ width: `${state.getThumbPercent(0) * 100}%` }}
              />
              {/* valueText is the announced value (aria-valuetext), not a second description. */}
              <SliderThumb inputRef={inputRef} className={styles.sliderThumb} />
            </>
          )}
        </SliderTrack>
      </Slider>
    </div>
  );
}
