import { act, render, screen } from '@testing-library/react';
import { Profiler, useLayoutEffect } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { useElementBounds } from './useElementBounds';

afterEach(() => vi.unstubAllGlobals());

it('measures the border box, supports content-box browsers and disconnects when detached', () => {
  let notify: ResizeObserverCallback = () => {};
  const observe = vi.fn();
  const disconnect = vi.fn();
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(callback: ResizeObserverCallback) {
        notify = callback;
      }
      observe = observe;
      disconnect = disconnect;
    },
  );
  function MeasuredNode({ shown = true }: { shown?: boolean }) {
    const [ref, bounds] = useElementBounds<HTMLDivElement>();
    return (
      <>
        {shown ? <div ref={ref} data-testid="node" /> : null}
        <output aria-label="Node size">
          {bounds.width} × {bounds.height}
        </output>
      </>
    );
  }
  const view = render(<MeasuredNode />);
  const node = screen.getByTestId('node');
  expect(observe).toHaveBeenCalledExactlyOnceWith(node);
  const emit = (borderBoxSize: ResizeObserverSize[]) => {
    const entry: ResizeObserverEntry = {
      target: node,
      borderBoxSize,
      contentBoxSize: [],
      devicePixelContentBoxSize: [],
      contentRect: new DOMRect(0, 0, 224, 116),
    };
    act(() => notify([entry], {} as ResizeObserver));
  };
  emit([{ inlineSize: 228, blockSize: 120 }]);
  expect(screen.getByRole('status', { name: 'Node size' })).toHaveTextContent('228 × 120');
  emit([]);
  expect(screen.getByRole('status', { name: 'Node size' })).toHaveTextContent('224 × 116');
  act(() => notify([], {} as ResizeObserver));
  expect(screen.getByRole('status')).toHaveTextContent('224 × 116');
  view.rerender(<MeasuredNode shown={false} />);
  expect(disconnect).toHaveBeenCalledOnce();
});

it('keeps repeated identical measurements idle without reporting changed bounds', () => {
  let notify: ResizeObserverCallback = () => {};
  const observe = vi.fn();
  const disconnect = vi.fn();
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(callback: ResizeObserverCallback) {
        notify = callback;
      }
      observe = observe;
      disconnect = disconnect;
    },
  );
  const onBounds = vi.fn();
  const onCommit = vi.fn();
  function MeasuredNode() {
    const [ref, bounds] = useElementBounds<HTMLDivElement>();
    useLayoutEffect(() => onBounds(bounds), [bounds]);
    return (
      <div ref={ref} data-testid="node">
        {bounds.width} × {bounds.height}
      </div>
    );
  }
  const view = render(
    <Profiler id="Measured node" onRender={onCommit}>
      <MeasuredNode />
    </Profiler>,
  );
  const node = screen.getByTestId('node');
  const emit = (width: number, height: number, border = true) => {
    const entry: ResizeObserverEntry = {
      target: node,
      borderBoxSize: border ? [{ inlineSize: width, blockSize: height }] : [],
      contentBoxSize: [],
      devicePixelContentBoxSize: [],
      contentRect: new DOMRect(0, 0, width, height),
    };
    act(() => notify([entry], {} as ResizeObserver));
  };
  emit(420, 240);
  expect(onBounds).toHaveBeenLastCalledWith({ width: 420, height: 240 });
  onBounds.mockClear();
  onCommit.mockClear();

  // Separate observer deliveries must remain idle, not merely React-batched.
  for (let delivery = 0; delivery < 32; delivery += 1) emit(420, 240);
  // React may commit its first same-state update attempt after the real resize;
  // consumers must remain unchanged and subsequent deliveries must be idle.
  expect(onCommit.mock.calls.length).toBeLessThanOrEqual(1);
  expect(onBounds).not.toHaveBeenCalled();
  expect(observe).toHaveBeenCalledExactlyOnceWith(node);
  onCommit.mockClear();
  for (let delivery = 0; delivery < 32; delivery += 1) emit(420, 240);
  expect(onCommit).not.toHaveBeenCalled();
  expect(onBounds).not.toHaveBeenCalled();

  // Fallback browser geometry with the same dimensions is also unchanged.
  emit(420, 240, false);
  act(() => notify([], {} as ResizeObserver));
  expect(onCommit).not.toHaveBeenCalled();
  expect(onBounds).not.toHaveBeenCalled();

  // Preserve real fractional changes on either axis; do not round geometry.
  emit(420.25, 240);
  expect(onCommit).toHaveBeenCalledOnce();
  expect(onBounds).toHaveBeenCalledExactlyOnceWith({ width: 420.25, height: 240 });
  onCommit.mockClear();
  onBounds.mockClear();
  emit(420.25, 240.5, false);
  expect(onCommit).toHaveBeenCalledOnce();
  expect(onBounds).toHaveBeenCalledExactlyOnceWith({ width: 420.25, height: 240.5 });
  expect(node).toHaveTextContent('420.25 × 240.5');
  view.unmount();
  expect(disconnect).toHaveBeenCalledOnce();
});

it('retains the latest real resize when observer deliveries are batched', () => {
  let notify: ResizeObserverCallback = () => {};
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(callback: ResizeObserverCallback) {
        notify = callback;
      }
      observe() {}
      disconnect() {}
    },
  );
  function MeasuredNode() {
    const [ref, bounds] = useElementBounds<HTMLDivElement>();
    return (
      <div ref={ref} data-testid="node">
        {bounds.width} × {bounds.height}
      </div>
    );
  }
  render(<MeasuredNode />);
  const node = screen.getByTestId('node');
  const entry = (width: number, height: number): ResizeObserverEntry => ({
    target: node,
    borderBoxSize: [{ inlineSize: width, blockSize: height }],
    contentBoxSize: [],
    devicePixelContentBoxSize: [],
    contentRect: new DOMRect(0, 0, width, height),
  });
  act(() => {
    notify([entry(320, 180)], {} as ResizeObserver);
    notify([entry(420.25, 180)], {} as ResizeObserver);
    notify([entry(420.25, 180)], {} as ResizeObserver);
    notify([entry(420.25, 240.5)], {} as ResizeObserver);
  });
  expect(node).toHaveTextContent('420.25 × 240.5');
});
