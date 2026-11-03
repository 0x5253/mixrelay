/** Continuous parameter range shared by every continuous control. */
export interface ParameterRange {
  minimum: number;
  maximum: number;
  step: number;
  scale?: 'linear' | 'logarithmic';
}

/** A logarithmic scale needs a positive, non-empty range; otherwise it is linear. */
export function isLogarithmicRange({ minimum, maximum, scale }: ParameterRange) {
  return scale === 'logarithmic' && minimum > 0 && maximum > minimum;
}

export function clampParameter(value: number, { minimum, maximum }: ParameterRange) {
  return Math.max(minimum, Math.min(maximum, value));
}

/** Rounds to the public step counted from the minimum, drops float noise, then clamps. */
export function snapParameter(value: number, range: ParameterRange) {
  const { minimum, step } = range;
  return clampParameter(
    Number((minimum + Math.round((value - minimum) / step) * step).toPrecision(12)),
    range,
  );
}

/** Position 0–1 along the control, logarithmic where the range allows it. */
export function parameterPosition(value: number, range: ParameterRange) {
  const { minimum, maximum } = range;
  const clamped = clampParameter(value, range);
  return isLogarithmicRange(range)
    ? Math.log(clamped / minimum) / Math.log(maximum / minimum)
    : (clamped - minimum) / (maximum - minimum);
}

/** Unsnapped value at a 0–1 position; callers snap only when they report a value. */
export function parameterAtPosition(position: number, range: ParameterRange) {
  const { minimum, maximum } = range;
  return isLogarithmicRange(range)
    ? minimum * (maximum / minimum) ** position
    : minimum + (maximum - minimum) * position;
}

/** Keyboard contract: arrows move one public step, Page Up/Down ten, Home/End the ends.
 * Text entry keeps Left/Right for caret movement by passing `vertical`. */
export function steppedParameterKey(
  key: string,
  value: number,
  { minimum, maximum, step }: ParameterRange,
  arrows: 'both' | 'vertical' = 'both',
): number | undefined {
  const changes: Record<string, number> = {
    ArrowUp: value + step,
    ArrowDown: value - step,
    PageUp: value + step * 10,
    PageDown: value - step * 10,
    Home: minimum,
    End: maximum,
  };
  if (arrows === 'both') {
    changes.ArrowRight = value + step;
    changes.ArrowLeft = value - step;
  }
  return changes[key];
}

/** An integer slider that stands for a range. A linear range is its own slider; a
 * logarithmic one gets enough integer positions that, where the curve is steepest, one
 * position is no more than one public step. */
export interface SliderScale {
  minValue: number;
  maxValue: number;
  step: number;
  toSlider(value: number): number;
  /** The snapped parameter value at a slider position. */
  fromSlider(position: number): number;
}

export function sliderScale(range: ParameterRange): SliderScale {
  const { minimum, maximum, step } = range;
  if (!isLogarithmicRange(range))
    return {
      minValue: minimum,
      maxValue: maximum,
      step,
      toSlider: (value) => value,
      fromSlider: (position) => position,
    };
  const resolution = Math.ceil((Math.log(maximum / minimum) * maximum) / step);
  return {
    minValue: 0,
    maxValue: resolution,
    step: 1,
    toSlider: (value) => parameterPosition(value, range) * resolution,
    fromSlider: (position) =>
      snapParameter(parameterAtPosition(position / resolution, range), range),
  };
}
