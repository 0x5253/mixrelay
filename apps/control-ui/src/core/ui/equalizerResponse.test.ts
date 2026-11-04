import { expect, it } from 'vitest';
import {
  equalizerDragChange,
  equalizerKeyChange,
  equalizerPlot,
  equalizerResponse,
  frequencyPosition,
  gainPosition,
  plotFrequency,
  plotGain,
  resolveEqualizerBands,
  responsePath,
} from './equalizerResponse';

it('plots the actual RBJ center gain at every supported rate, including the native cutoff clamp', () => {
  for (const rate of [44100, 48000, 96000, 192000, 384000]) {
    for (const frequency of [20, 150, 1000, 6000, 20000]) {
      for (const q of [0.1, Math.SQRT1_2, 10]) {
        for (const gain of [-24, -6, 0, 6, 24]) {
          expect(
            equalizerResponse(Math.min(frequency, rate * 0.45), [{ frequency, gain, q }], rate),
          ).toBeCloseTo(gain, 4);
        }
      }
    }
  }
});

it('shows the combined three-band response and responds to Q rather than drawing decorative bells', () => {
  const bands = [
    { frequency: 150, gain: 6, q: Math.SQRT1_2 },
    { frequency: 1000, gain: -4, q: 2 },
    { frequency: 6000, gain: 8, q: 1 },
  ];
  for (const probe of [20, 150, 800, 1000, 6000, 20000]) {
    expect(equalizerResponse(probe, bands, 48000)).toBeCloseTo(
      bands.reduce((sum, band) => sum + equalizerResponse(probe, [band], 48000), 0),
      10,
    );
  }
  expect(equalizerResponse(2000, [{ frequency: 1000, gain: 12, q: 0.1 }], 48000)).toBeGreaterThan(
    equalizerResponse(2000, [{ frequency: 1000, gain: 12, q: 10 }], 48000),
  );
});

it('maps graph positions to exact parameter units, with finite bounded SVG at the display edges', () => {
  for (const frequency of [20, 150, 1000, 6000, 19999, 20000])
    expect(plotFrequency(frequencyPosition(frequency))).toBe(frequency);
  for (const gain of [-24, -12, -0.1, 0, 0.1, 12, 24])
    expect(plotGain(gainPosition(gain))).toBe(gain);
  expect(plotFrequency(-100)).toBe(20);
  expect(plotFrequency(1000)).toBe(20000);
  expect(plotGain(-100)).toBe(24);
  expect(plotGain(1000)).toBe(-24);
  expect(gainPosition(0)).toBe((equalizerPlot.top + equalizerPlot.bottom) / 2);
  for (const gain of [-24, 0, 24]) {
    const path = responsePath(
      Array.from({ length: 3 }, () => ({ frequency: 1000, gain, q: 0.1 })),
      48000,
    );
    expect(path).not.toMatch(/NaN|Infinity/);
    expect(path.startsWith('M')).toBe(true);
    expect(path.split('L')).toHaveLength(257);
    for (const point of path.slice(1).split('L')) {
      const [x, y] = point.split(' ').map(Number);
      expect(x).toBeGreaterThanOrEqual(equalizerPlot.left);
      expect(x).toBeLessThanOrEqual(equalizerPlot.right);
      expect(y).toBeGreaterThanOrEqual(equalizerPlot.top);
      expect(y).toBeLessThanOrEqual(equalizerPlot.bottom);
    }
  }
});

const control = (id: string, value: number, step: number, minimum: number, maximum: number) => ({
  id,
  value,
  step,
  minimum,
  maximum,
});
const band = {
  id: 'mid',
  label: 'Mid',
  frequency: control('midFrequencyHz', 1000, 1, 20, 20_000),
  gain: control('midDb', 0, 0.1, -24, 24),
  q: control('midQ', 0.7, 0.001, 0.1, 10),
};

it('steps frequency with Left/Right and gain with Up/Down by one control step, within range', () => {
  expect(
    ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].map((key) => equalizerKeyChange(key, band)),
  ).toEqual([
    { id: 'midFrequencyHz', value: 999 },
    { id: 'midFrequencyHz', value: 1001 },
    { id: 'midDb', value: 0.1 },
    { id: 'midDb', value: -0.1 },
  ]);
  expect(equalizerKeyChange('Tab', band)).toBeUndefined();
  const top = { ...band, gain: control('midDb', 24, 0.1, -24, 24) };
  expect(equalizerKeyChange('ArrowUp', top)).toEqual({ id: 'midDb', value: 24 });
  // Decimal steps land on the step grid rather than accumulating float error.
  const fine = { ...band, gain: control('midDb', 0.2, 0.1, -24, 24) };
  expect(equalizerKeyChange('ArrowUp', fine)).toEqual({ id: 'midDb', value: 0.3 });
});

it('maps a dragged point to frequency and gain together', () => {
  const x = frequencyPosition(2200) / equalizerPlot.width;
  const y = gainPosition(6) / equalizerPlot.height;
  expect(equalizerDragChange(band, x, y)).toEqual([
    { id: 'midFrequencyHz', value: 2200 },
    { id: 'midDb', value: 6 },
  ]);
  expect(equalizerDragChange(band, -1, 2)).toEqual([
    { id: 'midFrequencyHz', value: 20 },
    { id: 'midDb', value: -24 },
  ]);
});

it('resolves only bands whose three controls exist', () => {
  const bands = [
    { id: 'mid', label: 'Mid', frequencyId: 'midFrequencyHz', gainId: 'midDb', qId: 'midQ' },
    { id: 'high', label: 'High', frequencyId: 'highFrequencyHz', gainId: 'highDb', qId: 'highQ' },
  ];
  expect(resolveEqualizerBands(bands, [band.frequency, band.gain, band.q])).toEqual([band]);
});
