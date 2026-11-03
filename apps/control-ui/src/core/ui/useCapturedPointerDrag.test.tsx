import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { useCapturedPointerDrag } from './useCapturedPointerDrag';

function DragProbe({
  disabled,
  begin,
  update,
}: {
  disabled?: boolean;
  begin: (y: number) => { y: number } | undefined;
  update: (state: { y: number }, y: number) => void;
}) {
  const handlers = useCapturedPointerDrag({
    disabled,
    begin: (event) => begin(event.clientY),
    update: (state, event) => update(state, event.clientY),
  });
  return <div data-testid="drag" {...handlers} />;
}

function captureSpies(element: HTMLElement) {
  const captured = new Set<number>();
  element.setPointerCapture = vi.fn((id: number) => void captured.add(id));
  element.hasPointerCapture = vi.fn((id: number) => captured.has(id));
  element.releasePointerCapture = vi.fn((id: number) => void captured.delete(id));
  return element;
}

it('captures one primary pointer, ignores others and releases on up, loss and unmount', () => {
  const begin = vi.fn((y: number) => ({ y }));
  const update = vi.fn();
  const view = render(<DragProbe begin={begin} update={update} />);
  const element = captureSpies(screen.getByTestId('drag'));

  fireEvent.pointerDown(element, { button: 2, pointerId: 1, clientY: 10 });
  expect(begin).not.toHaveBeenCalled();
  fireEvent.pointerDown(element, { button: 0, pointerId: 1, clientY: 10 });
  fireEvent.pointerDown(element, { button: 0, pointerId: 2, clientY: 10 });
  expect(begin).toHaveBeenCalledOnce();
  fireEvent.pointerMove(element, { pointerId: 2, clientY: 5 });
  fireEvent.pointerMove(element, { pointerId: 1, clientY: 4 });
  expect(update).toHaveBeenCalledExactlyOnceWith({ y: 10 }, 4);
  fireEvent.pointerUp(element, { pointerId: 2 });
  expect(element.releasePointerCapture).not.toHaveBeenCalled();
  fireEvent.pointerUp(element, { pointerId: 1 });
  expect(element.releasePointerCapture).toHaveBeenCalledWith(1);

  fireEvent.pointerDown(element, { button: 0, pointerId: 3, clientY: 0 });
  fireEvent.lostPointerCapture(element, { pointerId: 3 });
  fireEvent.pointerMove(element, { pointerId: 3, clientY: 1 });
  expect(update).toHaveBeenCalledOnce();

  fireEvent.pointerDown(element, { button: 0, pointerId: 4, clientY: 0 });
  view.unmount();
  expect(element.releasePointerCapture).toHaveBeenLastCalledWith(4);
});

it('ignores presses a control declines and ends an active drag when disabled', () => {
  const update = vi.fn();
  const view = render(<DragProbe begin={() => undefined} update={update} />);
  const element = captureSpies(screen.getByTestId('drag'));
  fireEvent.pointerDown(element, { button: 0, pointerId: 1, clientY: 0 });
  expect(element.setPointerCapture).not.toHaveBeenCalled();

  view.rerender(<DragProbe begin={(y) => ({ y })} update={update} />);
  fireEvent.pointerDown(element, { button: 0, pointerId: 5, clientY: 0 });
  view.rerender(<DragProbe disabled begin={(y) => ({ y })} update={update} />);
  expect(element.releasePointerCapture).toHaveBeenCalledWith(5);
  fireEvent.pointerDown(element, { button: 0, pointerId: 6, clientY: 0 });
  fireEvent.pointerMove(element, { pointerId: 6, clientY: 1 });
  expect(update).not.toHaveBeenCalled();
});
