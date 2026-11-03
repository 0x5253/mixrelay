import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { Knob, type KnobProps } from './Knob';

const defaults: KnobProps = {
  label: 'Gain',
  value: 0,
  min: -24,
  max: 24,
  step: 0.1,
  onChange: () => {},
};
function renderKnob(props: Partial<KnobProps> = {}) {
  const change = vi.fn();
  const view = render(<Knob {...defaults} {...props} onChange={change} />);
  const slider = screen.getByRole('slider');
  const dial = slider.parentElement as HTMLElement;
  dial.setPointerCapture = vi.fn();
  dial.hasPointerCapture = vi.fn(() => true);
  dial.releasePointerCapture = vi.fn();
  return { ...view, change, slider, dial };
}
it('exposes the actual value and unit without an internal integer position range', () => {
  const { slider } = renderKnob({
    value: Math.SQRT1_2,
    min: 0.1,
    max: 10,
    step: 0.01,
    valueText: '0.707 Q',
  });
  expect(slider).toHaveAttribute('aria-valuenow', String(Math.SQRT1_2));
  expect(slider).toHaveAttribute('min', '0.1');
  expect(slider).toHaveAttribute('max', '10');
  expect(slider).toHaveAttribute('aria-valuetext', '0.707 Q');
  expect(slider).toHaveAttribute('step', 'any');
});
it('supports arrows, page keys and endpoints in actual units with no graph shortcut propagation', () => {
  const { change, slider } = renderKnob();
  for (const [key, value] of [
    ['ArrowUp', 0.1],
    ['ArrowRight', 0.1],
    ['ArrowDown', -0.1],
    ['ArrowLeft', -0.1],
    ['PageUp', 1],
    ['PageDown', -1],
    ['Home', -24],
    ['End', 24],
  ] as const) {
    fireEvent.keyDown(slider, { key });
    expect(change).toHaveBeenLastCalledWith(value);
  }
  fireEvent.keyDown(slider, { key: 'Tab' });
  expect(change).toHaveBeenCalledTimes(8);
});
it('uses continuous drag position, fractional movements and Shift fine adjustment', () => {
  const { dial, change, slider } = renderKnob();
  fireEvent.pointerDown(dial, { button: 0, pointerId: 1, clientY: 100 });
  expect(slider).toHaveFocus();
  fireEvent.pointerMove(dial, { pointerId: 1, clientY: 99.5 });
  expect(change).toHaveBeenLastCalledWith(0.2);
  fireEvent.pointerMove(dial, { pointerId: 1, clientY: 99, shiftKey: true });
  fireEvent.pointerMove(dial, { pointerId: 1, clientY: 98.5, shiftKey: true });
  expect(change).toHaveBeenLastCalledWith(0.2);
  fireEvent.pointerMove(dial, { pointerId: 2, clientY: -1000 });
  expect(change).toHaveBeenCalledTimes(3);
  fireEvent.pointerCancel(dial, { pointerId: 1 });
  fireEvent.pointerMove(dial, { pointerId: 1, clientY: 0 });
  expect(change).toHaveBeenCalledTimes(3);
  expect(dial.releasePointerCapture).toHaveBeenCalledWith(1);
});
it('maps logarithmic frequency directly and keeps 1 Hz keyboard resolution', () => {
  const { change, slider, dial } = renderKnob({
    value: 20,
    min: 20,
    max: 20000,
    step: 1,
    scale: 'logarithmic',
  });
  fireEvent.pointerDown(dial, { button: 0, pointerId: 1, clientY: 100 });
  fireEvent.pointerMove(dial, { pointerId: 1, clientY: 20 });
  expect(change).toHaveBeenLastCalledWith(632);
  fireEvent.pointerUp(dial, { pointerId: 1 });
  fireEvent.keyDown(slider, { key: 'ArrowUp' });
  expect(change).toHaveBeenLastCalledWith(21);
});
it('keeps fine frequency drag at most 1 Hz per pixel even at the upper end', () => {
  const { change, dial } = renderKnob({
    value: 19900,
    min: 20,
    max: 20000,
    step: 1,
    scale: 'logarithmic',
  });
  fireEvent.pointerDown(dial, { button: 0, pointerId: 1, clientY: 100 });
  fireEvent.pointerMove(dial, { pointerId: 1, clientY: 99, shiftKey: true });
  expect(change).toHaveBeenLastCalledWith(19901);
  fireEvent.pointerMove(dial, { pointerId: 1, clientY: 98.75, shiftKey: true });
  fireEvent.pointerMove(dial, { pointerId: 1, clientY: 98.5, shiftKey: true });
  fireEvent.pointerMove(dial, { pointerId: 1, clientY: 98, shiftKey: true });
  expect(change).toHaveBeenLastCalledWith(19902);
});
it('ignores disabled/right/second pointer, cancels capture when disabled and unmounted', () => {
  const { dial, change, rerender, unmount } = renderKnob();
  fireEvent.pointerDown(dial, { button: 2, pointerId: 1, clientY: 100 });
  expect(dial.setPointerCapture).not.toHaveBeenCalled();
  fireEvent.pointerDown(dial, { button: 0, pointerId: 1, clientY: 100 });
  fireEvent.pointerDown(dial, { button: 0, pointerId: 2, clientY: 100 });
  expect(dial.setPointerCapture).toHaveBeenCalledTimes(1);
  rerender(<Knob {...defaults} disabled onChange={change} />);
  expect(dial.releasePointerCapture).toHaveBeenCalledWith(1);
  expect(screen.getByRole('slider')).toBeDisabled();
  fireEvent.pointerDown(dial, { button: 0, pointerId: 3, clientY: 100 });
  expect(dial.setPointerCapture).toHaveBeenCalledTimes(1);
  rerender(<Knob {...defaults} onChange={change} />);
  fireEvent.pointerDown(dial, { button: 0, pointerId: 4, clientY: 100 });
  unmount();
  expect(dial.releasePointerCapture).toHaveBeenLastCalledWith(4);
});
it('renders vector geometry and tracks controlled value changes', () => {
  function Demo() {
    const [value, setValue] = useState(0);
    return <Knob {...defaults} value={value} onChange={setValue} />;
  }
  render(<Demo />);
  const slider = screen.getByRole('slider');
  expect(slider.parentElement?.querySelector('svg')).toHaveAttribute('viewBox', '0 0 48 48');
  const focusArc = slider.parentElement?.querySelector('path');
  expect(focusArc).not.toHaveAttribute('vector-effect');
  expect(focusArc).toHaveAttribute('d', 'M7.737 40.263 A23 23 0 1 1 40.263 40.263');
  fireEvent.change(slider, { target: { value: '23.7' } });
  expect(slider).toHaveAttribute('aria-valuenow', '23.7');
  fireEvent.keyDown(slider, { key: 'End' });
  fireEvent.keyDown(slider, { key: 'ArrowUp' });
  expect(slider).toHaveAttribute('aria-valuenow', '24');
});
it('does not execute keys while disabled and defaults to whole steps for a unipolar dial', () => {
  const { slider, change, rerender } = renderKnob({
    value: -5,
    min: -10,
    max: 0,
    step: undefined,
    disabled: true,
  });
  fireEvent.keyDown(slider, { key: 'ArrowUp' });
  expect(change).not.toHaveBeenCalled();
  expect(slider.parentElement?.querySelectorAll('line')).toHaveLength(1);
  rerender(<Knob {...defaults} value={-5} min={-10} max={0} step={undefined} onChange={change} />);
  fireEvent.keyDown(slider, { key: 'ArrowUp' });
  expect(change).toHaveBeenLastCalledWith(-4);
});
it('keeps an active drag when a different pointer ends, and stops after capture loss', () => {
  const { dial, change } = renderKnob();
  fireEvent.pointerDown(dial, { button: 0, pointerId: 1, clientY: 100 });
  fireEvent.pointerUp(dial, { pointerId: 2 });
  fireEvent.pointerCancel(dial, { pointerId: 2 });
  fireEvent.pointerMove(dial, { pointerId: 1, clientY: 90 });
  expect(change).toHaveBeenLastCalledWith(3);
  fireEvent.lostPointerCapture(dial, { pointerId: 1 });
  fireEvent.pointerMove(dial, { pointerId: 1, clientY: 80 });
  expect(change).toHaveBeenCalledTimes(1);
});
it('exposes channel endpoints without adding duplicate focus targets', () => {
  const { dial, slider } = renderKnob({
    value: 0,
    min: -1,
    max: 1,
    step: 0.01,
    endpoints: { minimum: 'L', maximum: 'R' },
    valueText: '0 (L -1 / R 1)',
  });
  expect(slider).toHaveAccessibleName('Gain');
  expect(slider).toHaveAttribute('aria-valuetext', '0 (L -1 / R 1)');
  expect(screen.getByText('L').parentElement).toHaveAttribute('aria-hidden', 'true');
  expect(screen.getByText('R')).toBeInTheDocument();
  expect(dial.querySelectorAll('input')).toHaveLength(1);
});

it.each([
  { min: 0, max: 100, value: 0, length: 0, offset: 0, angle: -135 },
  { min: 0, max: 100, value: 1, length: 0.01, offset: 0, angle: -132.3 },
  { min: 0, max: 100, value: 50, length: 0.5, offset: 0, angle: 0 },
  { min: 0, max: 100, value: 100, length: 1, offset: 0, angle: 135 },
  { min: 1, max: 20, value: 4, length: 3 / 19, offset: 0, angle: -135 + 810 / 19 },
  { min: -24, max: 24, value: -24, length: 0.5, offset: 0, angle: -135 },
  { min: -24, max: 24, value: -6, length: 0.125, offset: -0.375, angle: -33.75 },
  { min: -24, max: 24, value: 0, length: 0, offset: -0.5, angle: 0 },
  { min: -24, max: 24, value: 6, length: 0.125, offset: -0.5, angle: 33.75 },
  { min: -24, max: 24, value: 24, length: 0.5, offset: -0.5, angle: 135 },
  { min: -60, max: 12, value: -60, length: 5 / 6, offset: 0, angle: -135 },
  { min: -60, max: 12, value: -30, length: 5 / 12, offset: -5 / 12, angle: -22.5 },
  { min: -60, max: 12, value: 0, length: 0, offset: -5 / 6, angle: 90 },
  { min: -60, max: 12, value: 6, length: 1 / 12, offset: -5 / 6, angle: 112.5 },
  { min: -60, max: 12, value: 12, length: 1 / 6, offset: -5 / 6, angle: 135 },
])('draws one continuous arc at $value in $min to $max', ({ length, offset, angle, ...range }) => {
  const { dial } = renderKnob(range);
  // Decorative SVG geometry has no accessible role; inspect it via its input owner.
  const arc = dial.querySelector('path[stroke-dasharray]');
  expect(dial.querySelector('circle')).toBeNull();
  expect(arc).toHaveAttribute('pathLength', '1');
  const [dash, gap] = arc?.getAttribute('stroke-dasharray')?.split(' ').map(Number) ?? [];
  expect(dash).toBeCloseTo(length);
  expect(gap).toBe(1);
  expect(Number(arc?.getAttribute('stroke-dashoffset'))).toBeCloseTo(offset);
  if (length === 0) expect(arc).toHaveAttribute('visibility', 'hidden');
  else expect(arc).not.toHaveAttribute('visibility');
  const pointer = dial.querySelector('line[class*=indicator]');
  expect(pointer).toHaveAttribute('transform', `rotate(${angle} 24 24)`);
  expect(pointer).toHaveAttribute('y1', '5');
  expect(pointer).toHaveAttribute('y2', '21');
  // Non-scaling strokes corrupt pathLength-normalised dashes in browser engines.
  expect(dial.querySelector('[vector-effect]')).toBeNull();
  if (range.min < 0 && range.max > 0) {
    expect(dial.querySelector('line[class*=center]')).toHaveAttribute(
      'transform',
      `rotate(${range.min === -60 ? 90 : 0} 24 24)`,
    );
  }
});

it.each([
  { min: -24, max: 24, value: -1, zero: 0.5, angle: 0, angleStep: 5.625, nextLength: 1 / 48 },
  { min: -60, max: 12, value: -1, zero: 5 / 6, angle: 90, angleStep: 3.75, nextLength: 1 / 72 },
  { min: 0, max: 100, value: 49, zero: 0, angle: 0, angleStep: 2.7, nextLength: 0.51 },
])('keeps keyboard steps, pointer and arc on the same scale for $min to $max', async (range) => {
  function Demo() {
    const [value, setValue] = useState(range.value);
    return <Knob {...defaults} {...range} step={1} value={value} onChange={setValue} />;
  }
  const user = userEvent.setup();
  render(<Demo />);
  const slider = screen.getByRole('slider');
  const dial = slider.parentElement;
  await user.tab();
  await user.keyboard('{ArrowUp}');
  expect(slider).toHaveAttribute('aria-valuenow', String(range.value + 1));
  // Geometry is decorative, so inspect it through the slider's owning dial.
  expect(dial?.querySelector('line[class*=indicator]')).toHaveAttribute(
    'transform',
    `rotate(${range.angle} 24 24)`,
  );
  const arc = dial?.querySelector('path[stroke-dasharray]');
  expect(Number(arc?.getAttribute('stroke-dashoffset'))).toBeCloseTo(-range.zero);
  const [length] = arc?.getAttribute('stroke-dasharray')?.split(' ').map(Number) ?? [];
  expect(length).toBe(range.zero ? 0 : 0.5);
  await user.keyboard('{ArrowUp}');
  const pointer = dial?.querySelector('line[class*=indicator]');
  const angle = Number(pointer?.getAttribute('transform')?.match(/rotate\(([^ ]+)/)?.[1]);
  expect(angle - range.angle).toBeCloseTo(range.angleStep);
  const [nextLength] = arc?.getAttribute('stroke-dasharray')?.split(' ').map(Number) ?? [];
  expect(nextLength).toBeCloseTo(range.nextLength);
  expect(Number(arc?.getAttribute('stroke-dashoffset'))).toBeCloseTo(-range.zero);
});
