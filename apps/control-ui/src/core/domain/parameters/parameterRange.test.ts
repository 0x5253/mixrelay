import { expect, it } from 'vitest';
import {
  clampParameter,
  isLogarithmicRange,
  parameterAtPosition,
  parameterPosition,
  sliderScale,
  snapParameter,
  steppedParameterKey,
} from './parameterRange';

const gain = { minimum: -60, maximum: 12, step: 0.5 };
const frequency = { minimum: 20, maximum: 20_000, step: 1, scale: 'logarithmic' as const };

it('uses a logarithmic scale only for a positive, non-empty range', () => {
  expect(isLogarithmicRange(frequency)).toBe(true);
  expect(isLogarithmicRange({ ...frequency, scale: 'linear' })).toBe(false);
  expect(isLogarithmicRange({ ...frequency, minimum: 0 })).toBe(false);
  expect(isLogarithmicRange({ ...frequency, maximum: 20 })).toBe(false);
});

it('snaps to the public step from the minimum without float noise and clamps', () => {
  expect(clampParameter(99, gain)).toBe(12);
  expect(snapParameter(-5.74, gain)).toBe(-5.5);
  expect(snapParameter(0.1 + 0.2, { minimum: 0, maximum: 1, step: 0.1 })).toBe(0.3);
  expect(snapParameter(-80, gain)).toBe(-60);
});

it('maps positions linearly or logarithmically and back', () => {
  expect(parameterPosition(-24, gain)).toBe(0.5);
  expect(parameterAtPosition(0.5, gain)).toBe(-24);
  expect(parameterPosition(632.455532, frequency)).toBeCloseTo(0.5, 6);
  expect(parameterAtPosition(0.5, frequency)).toBeCloseTo(632.455532, 5);
  expect(parameterPosition(1e9, frequency)).toBe(1);
});

it('steps by one unit, ten units or to the ends, keeping Left/Right for text carets', () => {
  expect(steppedParameterKey('ArrowUp', 0, gain)).toBe(0.5);
  expect(steppedParameterKey('ArrowLeft', 0, gain)).toBe(-0.5);
  expect(steppedParameterKey('PageDown', 0, gain)).toBe(-5);
  expect(steppedParameterKey('Home', 0, gain)).toBe(-60);
  expect(steppedParameterKey('End', 0, gain)).toBe(12);
  expect(steppedParameterKey('ArrowRight', 0, gain, 'vertical')).toBeUndefined();
  expect(steppedParameterKey('ArrowDown', 0, gain, 'vertical')).toBe(-0.5);
  expect(steppedParameterKey('Enter', 0, gain)).toBeUndefined();
});

it('snaps from a minimum that is not a step multiple', () => {
  const offset = { minimum: 0.5, maximum: 10.5, step: 1 };
  expect(snapParameter(3.2, offset)).toBe(3.5);
  expect(snapParameter(3.9, offset)).toBe(3.5);
  expect(snapParameter(4.1, offset)).toBe(4.5);
  expect(snapParameter(0, offset)).toBe(0.5);
});

it('gives a linear range its own slider', () => {
  const scale = sliderScale(gain);
  expect([scale.minValue, scale.maxValue, scale.step]).toEqual([-60, 12, 0.5]);
  expect(scale.toSlider(-6)).toBe(-6);
  expect(scale.fromSlider(-6)).toBe(-6);
});

it('gives a logarithmic range integer positions no coarser than one step at its top', () => {
  const scale = sliderScale(frequency);
  expect([scale.minValue, scale.step]).toEqual([0, 1]);
  expect(scale.maxValue).toBe(Math.ceil(Math.log(1000) * 20_000));
  expect(scale.fromSlider(scale.maxValue)).toBe(20_000);
  expect(scale.fromSlider(scale.maxValue - 1)).toBeGreaterThanOrEqual(19_999);
  expect(scale.fromSlider(0)).toBe(20);
  for (const value of [20, 440, 1000, 20_000])
    expect(scale.fromSlider(Math.round(scale.toSlider(value)))).toBe(value);
});

it('snaps a logarithmic slider value from its offset minimum', () => {
  const offset = sliderScale({ minimum: 2.5, maximum: 250.5, step: 1, scale: 'logarithmic' });
  const value = offset.fromSlider(Math.round(offset.maxValue / 2));
  expect((value - 2.5) % 1).toBe(0);
});
