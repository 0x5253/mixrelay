import { act, fireEvent, render, renderHook } from '@testing-library/react';
import type { PointerEvent as ReactPointerEvent } from 'react';
import { useRef } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { useAudioPortSelection } from './useAudioPortSelection';

function SelectionProbe({ onSelectEdges }: { onSelectEdges: (ids: readonly string[]) => void }) {
  const root = useRef<HTMLDivElement>(null);
  const selection = useAudioPortSelection(root, onSelectEdges);
  return (
    <div ref={root} tabIndex={-1} {...selection.captureProps} data-testid="root">
      <div className="react-flow__pane" data-testid="pane" />
      <div data-audio-port-side="output" data-audio-node-id="a" data-audio-port-id="a1" />
      <div data-audio-port-side="output" data-audio-node-id="a" data-audio-port-id="a2" />
      <div data-audio-port-side="output" data-audio-node-id="b" data-audio-port-id="b1" />
      <div data-audio-port-side="input" data-audio-node-id="b" data-audio-port-id="in" />
      <div data-audio-port-side="invalid" />
      <div data-audio-port-side="output" />
      <output>{JSON.stringify(selection.feedback)}</output>
    </div>
  );
}

function fixture() {
  const onSelectEdges = vi.fn();
  const view = render(<SelectionProbe onSelectEdges={onSelectEdges} />);
  const root = view.getByTestId('root');
  const pane = view.getByTestId('pane');
  const handles = Array.from(root.querySelectorAll<HTMLElement>('[data-audio-port-side]'));
  handles.forEach((handle, index) => {
    const x = index === 2 ? 50 : 10;
    vi.spyOn(handle, 'getBoundingClientRect').mockReturnValue(
      new DOMRect(x, index * 10 + 10, 2, 2),
    );
  });
  vi.spyOn(root, 'getBoundingClientRect').mockReturnValue(new DOMRect(2, 3, 200, 200));
  const feedback = () => JSON.parse(root.querySelector('output')?.textContent ?? '{}');
  const selected = () =>
    Array.from(root.querySelectorAll<HTMLElement>('[data-port-selected]')).map(
      (handle) => handle.dataset.audioPortId,
    );
  const event = (kind: string, x: number, y: number, extra: PointerEventInit = {}) => {
    fireEvent(pane, new PointerEvent(kind, { bubbles: true, clientX: x, clientY: y, ...extra }));
  };
  return { ...view, root, pane, handles, onSelectEdges, feedback, selected, event };
}

afterEach(() => vi.unstubAllGlobals());

describe('output port selection gestures', () => {
  it.each([
    { pointerType: 'touch' },
    { button: 1 },
    { isPrimary: false },
    { metaKey: true },
    { ctrlKey: true },
  ])('ignores gestures reserved for other canvas controls %j', (extra) => {
    const view = fixture();
    view.event('pointerdown', 0, 0, extra);
    view.event('pointermove', 100, 100);
    view.event('pointerup', 100, 100);
    expect(view.selected()).toEqual([]);
    expect(view.onSelectEdges).not.toHaveBeenCalled();
  });

  it('ignores control clicks, unrelated pointers and movements below the drag threshold', () => {
    const view = fixture();
    fireEvent.pointerDown(view.handles[0] as HTMLElement, { clientX: 0, clientY: 0 });
    view.event('pointermove', 100, 100);
    expect(view.feedback().position).toBeNull();
    view.event('pointerdown', 0, 0);
    expect(document.activeElement).toBe(view.root);
    view.event('pointermove', 100, 100, { pointerId: 2 });
    view.event('pointerup', 100, 100, { pointerId: 2 });
    expect(view.selected()).toEqual([]);
    view.event('pointermove', 1, 1);
    view.event('pointerup', 1, 1);
    expect(view.feedback().position).toBeNull();
  });
});

describe('committing and cancelling port selections', () => {
  it('keeps a selection from one source and restores it when Escape or pointer cancellation aborts a drag', () => {
    const view = fixture();
    view.event('pointerdown', 0, 0);
    view.event('pointermove', 20, 25);
    expect(view.feedback().position).toEqual({ x: 30, y: 34 });
    expect(view.selected()).toEqual(['a1', 'a2']);
    view.event('pointermove', 20, 25);
    view.event('pointerup', 20, 25);
    expect(view.feedback().position).toBeNull();
    view.event('pointerdown', 0, 0, { shiftKey: true });
    view.event('pointermove', 100, 100);
    expect(view.selected()).toEqual(['a1', 'a2']);
    fireEvent.keyDown(view.root, { key: 'ArrowDown' });
    expect(view.feedback().position).not.toBeNull();
    fireEvent.keyDown(view.root, { key: 'Escape' });
    expect(view.selected()).toEqual(['a1', 'a2']);
    view.event('pointerdown', 45, 25);
    view.event('pointermove', 60, 35);
    expect(view.selected()).toEqual(['b1']);
    view.event('pointercancel', 60, 35);
    expect(view.selected()).toEqual(['a1', 'a2']);
    fireEvent.keyDown(view.root, { key: 'Escape' });
    expect(view.selected()).toEqual([]);
  });

  it('can add ports with Shift and uses nearest source when several nodes overlap the rectangle', () => {
    const view = fixture();
    view.event('pointerdown', 0, 0, { shiftKey: true });
    view.event('pointermove', 20, 15, { shiftKey: true });
    view.event('pointerup', 20, 15, { shiftKey: true });
    expect(view.selected()).toEqual(['a1']);
    view.event('pointerdown', 0, 15, { shiftKey: true });
    view.event('pointermove', 60, 35, { shiftKey: true });
    view.event('pointerup', 60, 35, { shiftKey: true });
    expect(view.selected()).toEqual(['a1', 'a2']);
    view.event('pointerdown', 60, 35);
    view.event('pointermove', 0, 0);
    view.event('pointerup', 0, 0);
    expect(view.selected()).toEqual(['b1']);
    expect(view.onSelectEdges).toHaveBeenCalledWith([]);
  });

  it('safely handles a root being removed during an active gesture', () => {
    const root = { current: null };
    const onSelectEdges = vi.fn();
    const { result } = renderHook(() => useAudioPortSelection(root, onSelectEdges));
    const pane = document.createElement('div');
    pane.className = 'react-flow__pane';
    const event = {
      pointerType: 'mouse',
      button: 0,
      isPrimary: true,
      pointerId: 1,
      clientX: 0,
      clientY: 0,
      shiftKey: true,
      target: pane,
      currentTarget: pane,
    } as unknown as ReactPointerEvent<HTMLElement>;
    act(() => result.current.captureProps.onPointerDownCapture(event));
    act(() =>
      result.current.captureProps.onPointerMoveCapture({ ...event, clientX: 20, clientY: 20 }),
    );
    expect(result.current.feedback).toEqual({ ports: [], position: { x: 32, y: 32 } });
    act(() =>
      result.current.captureProps.onPointerUpCapture({ ...event, clientX: 20, clientY: 20 }),
    );
    expect(onSelectEdges).toHaveBeenCalledExactlyOnceWith([]);
    act(() => result.current.clear());
    expect(result.current.feedback).toEqual({ ports: [], position: null });
  });
});

class TestDOMPoint {
  constructor(
    public x = 0,
    public y = 0,
  ) {}
  matrixTransform() {
    return this;
  }
}

function edge(
  root: HTMLElement,
  id: string | null,
  points: [number, number][],
  bounds = new DOMRect(0, 0, 100, 100),
  transform: object | null = {},
) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  const group = document.createElementNS('http://www.w3.org/2000/svg', 'g');
  group.classList.add('react-flow__edge');
  if (id !== null) group.dataset.id = id;
  const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
  path.classList.add('react-flow__edge-path');
  Object.defineProperties(path, {
    getBoundingClientRect: { value: () => bounds },
    getScreenCTM: { value: () => transform },
    getTotalLength: { value: () => (points.length - 1) * 10 },
    getPointAtLength: {
      value: (length: number) => {
        const point = points[Math.round(length / 10)];
        return { x: point?.[0] ?? 0, y: point?.[1] ?? 0 };
      },
    },
  });
  group.append(path);
  svg.append(group);
  root.append(svg);
}

describe('Shift rectangle edge selection', () => {
  it.each([
    [
      'inside',
      [
        [12, 12],
        [14, 14],
      ],
    ],
    [
      'enters',
      [
        [15, 5],
        [15, 15],
      ],
    ],
    [
      'vertical crossing',
      [
        [15, 5],
        [15, 25],
      ],
    ],
    [
      'horizontal crossing',
      [
        [5, 15],
        [25, 15],
      ],
    ],
    [
      'bottom corner',
      [
        [15, 25],
        [25, 15],
      ],
    ],
    [
      'left corner',
      [
        [5, 15],
        [15, 25],
      ],
    ],
  ] as [string, [number, number][]][])(
    'selects a path %s even if neither endpoint is inside',
    (_name, points) => {
      vi.stubGlobal('DOMPoint', TestDOMPoint);
      const view = fixture();
      edge(view.root, 'edge-1', points);
      view.event('pointerdown', 10, 10, { shiftKey: true });
      view.event('pointerup', 20, 20, { shiftKey: true });
      expect(view.onSelectEdges).toHaveBeenCalledExactlyOnceWith(['edge-1']);
    },
  );
});

describe('ignoring nonselectable edge paths', () => {
  it.each([
    ['left', new DOMRect(-20, 0, 5, 5)],
    ['right', new DOMRect(30, 0, 5, 5)],
    ['above', new DOMRect(0, -20, 5, 5)],
    ['below', new DOMRect(0, 30, 5, 5)],
  ] as const)('ignores paths whose bounds are %s of the selection', (_name, bounds) => {
    vi.stubGlobal('DOMPoint', TestDOMPoint);
    const view = fixture();
    edge(
      view.root,
      'offscreen',
      [
        [12, 12],
        [14, 14],
      ],
      bounds,
    );
    view.event('pointerdown', 10, 10, { shiftKey: true });
    view.event('pointerup', 20, 20, { shiftKey: true });
    expect(view.onSelectEdges).toHaveBeenCalledExactlyOnceWith([]);
  });

  it('omits paths without a screen transform or identity and nonintersecting paths', () => {
    vi.stubGlobal('DOMPoint', TestDOMPoint);
    const view = fixture();
    edge(
      view.root,
      'no-transform',
      [
        [12, 12],
        [14, 14],
      ],
      undefined,
      null,
    );
    edge(view.root, null, [
      [12, 12],
      [14, 14],
    ]);
    edge(view.root, 'misses', [
      [25, 5],
      [35, 15],
    ]);
    edge(view.root, 'misses-second-segment', [
      [5, 5],
      [5, 15],
      [5, 25],
    ]);
    edge(view.root, 'diagonal-misses', [
      [9, 10],
      [10, 9],
    ]);
    view.event('pointerdown', 10, 10, { shiftKey: true });
    view.event('pointerup', 20, 20, { shiftKey: true });
    expect(view.onSelectEdges).toHaveBeenCalledExactlyOnceWith([]);
  });
});
