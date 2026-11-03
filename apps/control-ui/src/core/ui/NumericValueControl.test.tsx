import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { NumericValueControl, type NumericValueControlProps } from './NumericValueControl';

const base: NumericValueControlProps = {
  label: 'Frequency',
  value: 1000,
  minimum: 20,
  maximum: 20000,
  step: 1,
  scale: 'logarithmic',
  suffix: 'Hz',
  onChange: () => {},
};
function setup(props: Partial<NumericValueControlProps> = {}) {
  const change = vi.fn();
  const activate = vi.fn();
  const view = render(
    <MixRelayProvider>
      <NumericValueControl {...base} {...props} onChange={change} onActivate={activate} />
    </MixRelayProvider>,
  );
  const input = within(view.container).getByRole('textbox');
  const root = input.closest('.nodrag') as HTMLDivElement;
  root.setPointerCapture = vi.fn();
  root.hasPointerCapture = vi.fn(() => true);
  root.releasePointerCapture = vi.fn();
  return { ...view, input, root, change, activate };
}
it('has one named exact input with centered content-width value/unit and room for longer entry', () => {
  const { input, change, activate } = setup({ value: 150 });
  expect(input).toHaveAccessibleName('Frequency');
  expect(input).toHaveAccessibleDescription('Hz');
  expect((input as HTMLInputElement).style.width).toBe('3ch');
  fireEvent.focus(input);
  expect(activate).toHaveBeenCalledOnce();
  fireEvent.change(input, { target: { value: '20000' } });
  expect((input as HTMLInputElement).style.width).toBe('5ch');
  fireEvent.blur(input);
  expect(change).toHaveBeenLastCalledWith(20000);
});
it('commits actual fine decimal values and ignores cleared invalid entry', () => {
  const { input, change } = setup({
    label: 'Q',
    value: Number(Math.SQRT1_2.toFixed(3)),
    minimum: 0.1,
    maximum: 10,
    step: 0.001,
    suffix: '',
    scale: 'linear',
  });
  expect(input).toHaveAccessibleDescription('');
  fireEvent.focus(input);
  fireEvent.change(input, { target: { value: '' } });
  fireEvent.blur(input);
  expect(change).not.toHaveBeenCalled();
  fireEvent.focus(input);
  fireEvent.change(input, { target: { value: '2.003' } });
  fireEvent.blur(input);
  expect(change).toHaveBeenLastCalledWith(2.003);
});
it('uses real-unit keys and endpoints, keeps Shift arrows usable and stops graph key handling', () => {
  const parentKey = vi.fn();
  const change = vi.fn();
  render(
    <div role="application" aria-label="Graph keyboard boundary" onKeyDown={parentKey}>
      <NumericValueControl {...base} onChange={change} />
    </div>,
  );
  const input = screen.getByRole('textbox');
  for (const [key, value] of [
    ['ArrowUp', 1001],
    ['ArrowDown', 999],
    ['PageUp', 1010],
    ['PageDown', 990],
    ['Home', 20],
    ['End', 20000],
  ] as const) {
    fireEvent.keyDown(input, { key });
    expect(change).toHaveBeenLastCalledWith(value);
  }
  fireEvent.keyDown(input, { key: 'ArrowUp', shiftKey: true });
  expect(change).toHaveBeenLastCalledWith(1001);
  expect(parentKey).not.toHaveBeenCalled();
  fireEvent.keyDown(input, { key: 'ArrowLeft' });
  fireEvent.keyDown(input, { key: 'ArrowRight' });
  fireEvent.keyDown(input, { key: 'Tab' });
  expect(parentKey).toHaveBeenCalledTimes(3);
});
it('accumulates fractional linear drag, ignores other pointers and releases cancellation', () => {
  const { input, root, change } = setup({
    value: 0,
    minimum: -24,
    maximum: 24,
    step: 0.1,
    scale: 'linear',
  });
  fireEvent.pointerDown(root, { button: 2, pointerId: 1 });
  expect(root.setPointerCapture).not.toHaveBeenCalled();
  fireEvent.pointerDown(root, { button: 0, pointerId: 1, clientY: 100 });
  expect(input).toHaveFocus();
  fireEvent.pointerDown(root, { button: 0, pointerId: 2, clientY: 100 });
  expect(root.setPointerCapture).toHaveBeenCalledOnce();
  fireEvent.pointerMove(root, { pointerId: 2, clientY: 0 });
  fireEvent.pointerMove(root, { pointerId: 1, clientY: 100 });
  expect(change).not.toHaveBeenCalled();
  fireEvent.pointerMove(root, { pointerId: 1, clientY: 99.5 });
  expect(change).toHaveBeenLastCalledWith(0.1);
  fireEvent.pointerMove(root, { pointerId: 1, clientY: 99 });
  expect(change).toHaveBeenLastCalledWith(0.1);
  fireEvent.pointerUp(root, { pointerId: 2 });
  expect(root.releasePointerCapture).not.toHaveBeenCalled();
  fireEvent.pointerCancel(root, { pointerId: 1 });
  expect(root.releasePointerCapture).toHaveBeenCalledWith(1);
  fireEvent.pointerMove(root, { pointerId: 1, clientY: 0 });
  expect(change).toHaveBeenCalledTimes(2);
});
it('preserves continuous logarithmic drag and public 1Hz precision at the upper end', () => {
  const { root, change } = setup({ value: 20 });
  fireEvent.pointerDown(root, { button: 0, pointerId: 1, clientY: 200 });
  fireEvent.pointerMove(root, { pointerId: 1, clientY: 80 });
  expect(change).toHaveBeenLastCalledWith(632);
  fireEvent.pointerUp(root, { pointerId: 1 });
  const view = setup({ value: 19900 });
  fireEvent.pointerDown(view.root, { button: 0, pointerId: 2, clientY: 100 });
  for (let index = 1; index <= 10; index++)
    fireEvent.pointerMove(view.root, { pointerId: 2, clientY: 100 - index, shiftKey: true });
  expect(view.change).toHaveBeenLastCalledWith(19901);
});
it('releases pointer capture on Off and unmount and refuses all disabled edits', () => {
  const { root, input, change, rerender, unmount } = setup();
  fireEvent.pointerDown(root, { button: 0, pointerId: 3, clientY: 100 });
  rerender(<NumericValueControl {...base} disabled onChange={change} />);
  expect(root.releasePointerCapture).toHaveBeenCalledWith(3);
  expect(screen.getByRole('textbox')).toBeDisabled();
  const disabledRoot = screen.getByRole('textbox').closest('.nodrag') as HTMLElement;
  fireEvent.pointerDown(disabledRoot, { button: 0, pointerId: 4, clientY: 100 });
  fireEvent.keyDown(screen.getByRole('textbox'), { key: 'ArrowUp' });
  fireEvent.wheel(disabledRoot, { deltaY: -1 });
  expect(change).not.toHaveBeenCalled();
  unmount();
  expect(input).not.toBeInTheDocument();
});
it('releases an active pointer when unmounted without leaking capture and handles a lost capture', () => {
  const { root, unmount } = setup();
  fireEvent.pointerDown(root, { button: 0, pointerId: 4, clientY: 100 });
  unmount();
  expect(root.releasePointerCapture).toHaveBeenCalledWith(4);
  const view = setup();
  view.root.hasPointerCapture = vi.fn(() => false);
  fireEvent.pointerDown(view.root, { button: 0, pointerId: 5, clientY: 100 });
  fireEvent.lostPointerCapture(view.root);
  fireEvent.pointerMove(view.root, { pointerId: 5, clientY: 0 });
  expect(view.root.releasePointerCapture).not.toHaveBeenCalled();
  expect(view.change).not.toHaveBeenCalled();
});
it('changes one actual step per wheel tick, preserves batched events and syncs external changes', () => {
  const { root, change, rerender, activate } = setup({ value: 150 });
  const outer = vi.fn();
  document.addEventListener('wheel', outer);
  act(() => {
    for (let i = 0; i < 3; i++)
      root.dispatchEvent(
        new WheelEvent('wheel', { deltaY: -100, bubbles: true, cancelable: true }),
      );
  });
  expect(change.mock.calls.map(([value]) => value)).toEqual([151, 152, 153]);
  expect(outer).not.toHaveBeenCalled();
  expect(activate).toHaveBeenCalledTimes(3);
  rerender(<NumericValueControl {...base} value={20000} onChange={change} />);
  fireEvent.wheel(screen.getByRole('textbox'), { deltaY: 1 });
  expect(change).toHaveBeenLastCalledWith(19999);
  document.removeEventListener('wheel', outer);
});
it('accumulates Shift wheel substeps and removes its local nonpassive listener on unmount', () => {
  const add = vi.spyOn(HTMLElement.prototype, 'addEventListener');
  const remove = vi.spyOn(HTMLElement.prototype, 'removeEventListener');
  const { root, change, unmount } = setup({
    value: Number(Math.SQRT1_2.toFixed(3)),
    minimum: 0.1,
    maximum: 10,
    step: 0.001,
    scale: 'linear',
    suffix: '',
  });
  const wheel = add.mock.calls.find(
    ([type, , options]) =>
      type === 'wheel' && (options as AddEventListenerOptions)?.passive === false,
  )?.[1];
  fireEvent.wheel(root, { deltaY: 0 });
  expect(change).not.toHaveBeenCalled();
  for (let i = 0; i < 9; i++) fireEvent.wheel(root, { deltaY: -1, shiftKey: true });
  expect(change).not.toHaveBeenCalled();
  fireEvent.wheel(root, { deltaY: -1, shiftKey: true });
  expect(change).toHaveBeenLastCalledWith(0.708);
  unmount();
  expect(remove).toHaveBeenCalledWith('wheel', wheel);
  fireEvent.wheel(root, { deltaY: -1 });
  expect(change).toHaveBeenCalledOnce();
  add.mockRestore();
  remove.mockRestore();
});
it('shows the committed value after repeated wheel updates in one React batch', () => {
  function Controlled() {
    const [value, setValue] = useState(20);
    return <NumericValueControl {...base} value={value} onChange={setValue} />;
  }
  render(<Controlled />);
  const input = screen.getByRole('textbox');
  act(() => {
    for (let i = 0; i < 3; i++)
      input.dispatchEvent(new WheelEvent('wheel', { deltaY: -1, bubbles: true, cancelable: true }));
  });
  expect(input).toHaveValue('23');
});

it('grows the focus border with live signed decimal text without reserving extra width', () => {
  const { input, change } = setup({
    label: 'Gain',
    value: 0,
    minimum: -24,
    maximum: 24,
    step: 0.1,
    scale: 'linear',
    suffix: 'dB',
  });
  expect((input as HTMLInputElement).style.width).toBe('1ch');
  fireEvent.focus(input);
  fireEvent.change(input, { target: { value: '-23.7' } });
  expect((input as HTMLInputElement).style.width).toBe('5ch');
  fireEvent.blur(input);
  expect(change).toHaveBeenLastCalledWith(-23.7);
});
