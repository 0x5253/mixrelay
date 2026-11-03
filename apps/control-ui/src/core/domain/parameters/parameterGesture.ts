import {
  clampParameter,
  isLogarithmicRange,
  type ParameterRange,
  parameterAtPosition,
  parameterPosition,
} from './parameterRange';

/** Pixels of vertical travel for the whole knob. */
export const KNOB_TRAVEL_PX = 160;
/** With Shift, the knob range spans this many pixels, but never more than one step a pixel. */
export const KNOB_FINE_TRAVEL_PX = 1600;
/** A logarithmic numeric drag moves the value by its full ratio over this many pixels. */
export const NUMERIC_LOG_TRAVEL_PX = 240;
/** Shift moves numeric drags and wheel turns at this fraction of a step. */
export const FINE_STEP_FRACTION = 0.1;

/** The knob's continuous 0–1 position after `pixels` of upward travel. Fine travel moves one
 * public step (or less) per pixel, also at the top of logarithmic ranges. */
export function knobDragPosition(
  position: number,
  pixels: number,
  fine: boolean,
  range: ParameterRange,
) {
  const { minimum, maximum, step } = range;
  let next = position + pixels / KNOB_TRAVEL_PX;
  if (fine) {
    const value = parameterAtPosition(position, range);
    const sensitivity = isLogarithmicRange(range)
      ? (value * Math.log(maximum / minimum)) / KNOB_FINE_TRAVEL_PX
      : (maximum - minimum) / KNOB_FINE_TRAVEL_PX;
    next = parameterPosition(value + pixels * Math.min(step, sensitivity), range);
  }
  return Math.max(0, Math.min(1, next));
}

/** The unsnapped numeric value after `pixels` of upward travel. Logarithmic ranges move by
 * a continuous ratio; linear ranges and fine travel move in public steps. */
export function numericDragValue(
  value: number,
  pixels: number,
  fine: boolean,
  range: ParameterRange,
) {
  const next =
    isLogarithmicRange(range) && !fine
      ? value * (range.maximum / range.minimum) ** (pixels / NUMERIC_LOG_TRAVEL_PX)
      : value + pixels * range.step * (fine ? FINE_STEP_FRACTION : 1);
  return clampParameter(next, range);
}

/** Wheel notches accumulate until they make whole steps; fine notches count a tenth. */
export function wheelSteps(remainder: number, deltaY: number, fine: boolean) {
  if (deltaY === 0) return { remainder, steps: 0 };
  const total = remainder + Math.sign(-deltaY) * (fine ? FINE_STEP_FRACTION : 1);
  // `|| 0` keeps a cancelled turn at 0 rather than -0.
  const steps = Math.trunc(Number(total.toPrecision(12))) || 0;
  return { remainder: total - steps, steps };
}
