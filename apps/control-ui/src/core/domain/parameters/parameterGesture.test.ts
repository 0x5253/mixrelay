import { describe, expect, it } from 'vitest';
import {
  KNOB_TRAVEL_PX,
  knobDragPosition,
  NUMERIC_LOG_TRAVEL_PX,
  numericDragValue,
  wheelSteps,
} from './parameterGesture';
import { parameterAtPosition, parameterPosition } from './parameterRange';

const gain = { minimum: -60, maximum: 12, step: 0.5 };
const frequency = { minimum: 20, maximum: 20_000, step: 1, scale: 'logarithmic' as const };

describe('knob drag', () => {
  it('crosses the whole range over its travel and stays within it', () => {
    expect(knobDragPosition(0, KNOB_TRAVEL_PX, false, gain)).toBe(1);
    expect(knobDragPosition(0.5, KNOB_TRAVEL_PX / 4, false, gain)).toBe(0.75);
    expect(knobDragPosition(0.5, -KNOB_TRAVEL_PX, false, gain)).toBe(0);
    expect(knobDragPosition(0.9, KNOB_TRAVEL_PX, false, gain)).toBe(1);
  });

  it('moves at most one step a pixel when fine, also at the top of a logarithmic range', () => {
    const linear = knobDragPosition(0.5, 1, true, gain);
    expect(parameterAtPosition(linear, gain) - parameterAtPosition(0.5, gain)).toBeCloseTo(0.045);
    const top = parameterPosition(19_000, frequency);
    const moved = parameterAtPosition(knobDragPosition(top, 1, true, frequency), frequency);
    expect(moved - 19_000).toBeCloseTo(1, 6);
    const bottom = parameterPosition(30, frequency);
    // Near the bottom the same travel is finer than one step: the value's own share.
    const slow = parameterAtPosition(knobDragPosition(bottom, 1, true, frequency), frequency);
    expect(slow - 30).toBeCloseTo((30 * Math.log(1000)) / 1600, 6);
    expect(slow).toBeGreaterThan(30);
  });
});

describe('numeric drag', () => {
  it('moves linear ranges a step a pixel, a tenth when fine, and clamps', () => {
    expect(numericDragValue(0, 3, false, gain)).toBe(1.5);
    expect(numericDragValue(0, 3, true, gain)).toBeCloseTo(0.15);
    expect(numericDragValue(0, 1000, false, gain)).toBe(12);
  });

  it('moves logarithmic ranges by the full ratio over its travel, or in fine steps', () => {
    expect(numericDragValue(20, NUMERIC_LOG_TRAVEL_PX, false, frequency)).toBeCloseTo(20_000);
    expect(numericDragValue(200, NUMERIC_LOG_TRAVEL_PX / 3, false, frequency)).toBeCloseTo(2000);
    expect(numericDragValue(200, 10, true, frequency)).toBe(201);
  });
});

describe('wheel', () => {
  it('turns whole notches into steps in their direction', () => {
    expect(wheelSteps(0, -120, false)).toEqual({ remainder: 0, steps: 1 });
    expect(wheelSteps(0, 40, false)).toEqual({ remainder: 0, steps: -1 });
  });

  it('accumulates fine notches into one step after ten, without float drift', () => {
    let state = { remainder: 0, steps: 0 };
    const steps: number[] = [];
    for (let notch = 0; notch < 10; notch++) {
      state = wheelSteps(state.remainder, -1, true);
      steps.push(state.steps);
    }
    expect(steps).toEqual([0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    expect(Math.abs(state.remainder)).toBeLessThan(1e-9);
  });

  it('a reverse notch cancels accumulated fine notches and a still wheel changes nothing', () => {
    const forward = wheelSteps(wheelSteps(0, -1, true).remainder, -1, true);
    expect(forward.steps).toBe(0);
    expect(wheelSteps(forward.remainder, 1, false)).toMatchObject({ steps: 0 });
    expect(wheelSteps(0.9999999999999, 0, false)).toEqual({ remainder: 0.9999999999999, steps: 0 });
  });
});
