export interface EqualizerBandValue {
  frequency: number;
  gain: number;
  q: number;
}

/** Same RBJ peaking coefficients and 0.45 × sample-rate cutoff as the native DSP. */
export function equalizerResponse(
  frequency: number,
  bands: readonly EqualizerBandValue[],
  rate: number,
) {
  const probe = (2 * Math.PI * frequency) / rate;
  return bands.reduce((sum, band) => {
    const omega = (2 * Math.PI * Math.min(band.frequency, rate * 0.45)) / rate;
    const alpha = Math.sin(omega) / (2 * band.q);
    const amplitude = 10 ** (band.gain / 40);
    const numerator = [1 + alpha * amplitude, -2 * Math.cos(omega), 1 - alpha * amplitude];
    const denominator = [1 + alpha / amplitude, -2 * Math.cos(omega), 1 - alpha / amplitude];
    const magnitude = (coefficients: number[]) => {
      const real = coefficients.reduce(
        (value, coefficient, index) => value + coefficient * Math.cos(probe * index),
        0,
      );
      const imaginary = coefficients.reduce(
        (value, coefficient, index) => value - coefficient * Math.sin(probe * index),
        0,
      );
      return real * real + imaginary * imaginary;
    };
    return sum + 10 * Math.log10(magnitude(numerator) / magnitude(denominator));
  }, 0);
}

export const equalizerPlot = { left: 26, right: 294, top: 8, bottom: 96, width: 300, height: 120 };
export const frequencyPosition = (frequency: number) =>
  equalizerPlot.left +
  (Math.log(frequency / 20) / Math.log(1000)) * (equalizerPlot.right - equalizerPlot.left);
export const gainPosition = (gain: number) =>
  equalizerPlot.top + ((24 - gain) / 48) * (equalizerPlot.bottom - equalizerPlot.top);
export const plotFrequency = (position: number) =>
  Math.max(
    20,
    Math.min(
      20000,
      Math.round(
        20 * 1000 ** ((position - equalizerPlot.left) / (equalizerPlot.right - equalizerPlot.left)),
      ),
    ),
  );
export const plotGain = (position: number) =>
  Math.max(
    -24,
    Math.min(
      24,
      Number(
        (
          24 -
          ((position - equalizerPlot.top) / (equalizerPlot.bottom - equalizerPlot.top)) * 48
        ).toFixed(1),
      ),
    ),
  );

export function responsePath(bands: readonly EqualizerBandValue[], rate: number) {
  return Array.from({ length: 257 }, (_, index) => {
    const frequency = 20 * 1000 ** (index / 256);
    const gain = Math.max(-24, Math.min(24, equalizerResponse(frequency, bands, rate)));
    return `${index ? 'L' : 'M'}${frequencyPosition(frequency).toFixed(3)} ${gainPosition(gain).toFixed(3)}`;
  }).join(' ');
}

/** The part of a parameter control the graph reads and edits. */
export interface EqualizerControl {
  id: string;
  value: number;
  step: number;
  minimum: number;
  maximum: number;
}

/** One band's three controls, found by the ids its contract names. */
export interface EqualizerBand<C extends EqualizerControl> {
  id: string;
  label: string;
  frequency: C;
  gain: C;
  q: C;
}

/** Bands whose three controls all exist; an incomplete band is omitted, not invented. */
export function resolveEqualizerBands<C extends EqualizerControl>(
  bands: readonly { id: string; label: string; frequencyId: string; gainId: string; qId: string }[],
  controls: readonly C[],
): EqualizerBand<C>[] {
  return bands.flatMap((band) => {
    const frequency = controls.find((control) => control.id === band.frequencyId);
    const gain = controls.find((control) => control.id === band.gainId);
    const q = controls.find((control) => control.id === band.qId);
    return frequency && gain && q ? [{ id: band.id, label: band.label, frequency, gain, q }] : [];
  });
}

/** Left/Right step frequency and Up/Down step gain by one control step, within range. */
export function equalizerKeyChange(
  key: string,
  band: EqualizerBand<EqualizerControl>,
): { id: string; value: number } | undefined {
  const changes: Record<string, [EqualizerControl, number]> = {
    ArrowLeft: [band.frequency, band.frequency.value - band.frequency.step],
    ArrowRight: [band.frequency, band.frequency.value + band.frequency.step],
    ArrowUp: [band.gain, band.gain.value + band.gain.step],
    ArrowDown: [band.gain, band.gain.value - band.gain.step],
  };
  const change = changes[key];
  if (!change) return undefined;
  const [control, value] = change;
  return {
    id: control.id,
    value: Math.max(control.minimum, Math.min(control.maximum, Number(value.toPrecision(12)))),
  };
}

/** A handle dragged to a point given as fractions of the plot's width and height moves
 * the band's frequency and gain together. */
export function equalizerDragChange(
  band: EqualizerBand<EqualizerControl>,
  x: number,
  y: number,
): [{ id: string; value: number }, { id: string; value: number }] {
  return [
    { id: band.frequency.id, value: plotFrequency(x * equalizerPlot.width) },
    { id: band.gain.id, value: plotGain(y * equalizerPlot.height) },
  ];
}
