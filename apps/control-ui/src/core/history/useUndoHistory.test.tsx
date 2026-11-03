import { act, renderHook } from '@testing-library/react';
import { StrictMode, useEffect } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { fakeScheduler } from '../../test-support/fakeScheduler';
import { useUndoHistory } from './useUndoHistory';

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

it('keeps one history with stable methods across renders and Strict Mode', () => {
  const { result, rerender } = renderHook(() => useUndoHistory<number, string>(), {
    wrapper: StrictMode,
  });
  const first = result.current;
  act(() => result.current.record(0, 1, 'layout'));
  rerender();
  expect(result.current).toBe(first);
  act(() => {
    result.current.recordDebounced(1, 2, 'level', 'gain');
    result.current.flush();
    result.current.rebase((value) => value * 10);
  });
  expect(result.current.undo()).toEqual({ kind: 'level', value: 10 });
  expect(result.current.redo()).toEqual({ kind: 'level', value: 20 });
});

it('uses browser timers by default and a supplied clock and scheduler when given', () => {
  const browser = renderHook(() => useUndoHistory<number, string>());
  act(() => browser.result.current.recordDebounced(0, 1, 'level', 'gain'));
  expect(vi.getTimerCount()).toBe(1);
  act(() => vi.advanceTimersByTime(400));
  expect(browser.result.current.undo()?.value).toBe(0);
  const time = fakeScheduler();
  const supplied = renderHook(() => useUndoHistory<number, string>(time, time));
  act(() => supplied.result.current.recordDebounced(0, 1, 'level', 'gain'));
  expect([vi.getTimerCount(), time.pending()]).toEqual([0, 1]);
});

it('clears pending and committed entries when the owning control port changes', () => {
  const { result, rerender } = renderHook(
    ({ controlPort }) => {
      const history = useUndoHistory<number, string>();
      useEffect(() => {
        void controlPort;
        history.clear();
      }, [controlPort, history.clear]);
      return history;
    },
    { initialProps: { controlPort: {} } },
  );
  act(() => {
    result.current.record(0, 1, 'graph');
    result.current.recordDebounced(1, 2, 'level', 'gain');
  });
  rerender({ controlPort: {} });
  expect(vi.getTimerCount()).toBe(0);
  expect(result.current.undo()).toBeNull();
});

it('removes a pending timer on unmount instead of committing in the background', () => {
  const { result, unmount } = renderHook(() => useUndoHistory<number, string>());
  act(() => result.current.recordDebounced(0, 1, 'level', 'gain'));
  expect(vi.getTimerCount()).toBe(1);
  unmount();
  expect(vi.getTimerCount()).toBe(0);
});
