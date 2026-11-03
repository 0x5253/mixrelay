import { expect, it } from 'vitest';
import {
  equalizerPlot,
  equalizerResponse,
  frequencyPosition,
  gainPosition,
  plotFrequency,
  plotGain,
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
