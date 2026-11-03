import { act, render, renderHook } from '@testing-library/react';
import { Suspense, startTransition, useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { feedback } from '../theme/tokens';
import { useSustainedProblem } from './useSustainedProblem';

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

const isProblem = (value: string) => value !== 'ok';

it('hides a problem shorter than the delay and shows recovery at once', () => {
  const view = renderHook(({ value }) => useSustainedProblem(value, isProblem), {
    initialProps: { value: 'ok' },
  });
  view.rerender({ value: 'applying' });
  expect(view.result.current).toBe('ok');
  act(() => vi.advanceTimersByTime(feedback.problemDelay - 100));
  expect(view.result.current).toBe('ok');
  view.rerender({ value: 'ok' });
  act(() => vi.advanceTimersByTime(500));
  expect(view.result.current).toBe('ok');
});

it('shows a problem once it has lasted the delay, timed from when it began', () => {
  const view = renderHook(({ value }) => useSustainedProblem(value, isProblem), {
    initialProps: { value: 'ok' },
  });
  view.rerender({ value: 'applying' });
  act(() => vi.advanceTimersByTime(600));
  // A different problem continues the same outage rather than restarting the wait.
  view.rerender({ value: 'loading' });
  expect(view.result.current).toBe('ok');
  act(() => vi.advanceTimersByTime(400));
  expect(view.result.current).toBe('loading');
});

it('shows a problem immediately when nothing healthy has been seen', () => {
  const view = renderHook(() => useSustainedProblem('loading', isProblem));
  expect(view.result.current).toBe('loading');
});

it('ignores a render that React discards before commit', async () => {
  const pending = new Promise<never>(() => undefined);
  let update!: (value: string) => void;
  let shown = '';
  function Probe() {
    const [value, setValue] = useState('ok');
    update = setValue;
    shown = useSustainedProblem(value, (current) => current.startsWith('fail'));
    if (value === 'ok-suspended') throw pending;
    return null;
  }
  render(
    <Suspense fallback={null}>
      <Probe />
    </Suspense>,
  );
  act(() => update('fail'));
  expect(shown).toBe('ok');
  act(() => vi.advanceTimersByTime(600));
  // A transition that suspends is discarded; its healthy value was never shown.
  act(() => startTransition(() => update('ok-suspended')));
  act(() => update('fail-again'));
  act(() => vi.advanceTimersByTime(400));
  expect(shown).toBe('fail-again');
});
