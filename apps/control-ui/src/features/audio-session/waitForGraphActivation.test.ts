import { afterEach, expect, it, vi } from 'vitest';
import { monotonicClock, timerScheduler } from '../../core/adapters/browser/timers';
import { waitForGraphActivation } from './application/waitForGraphActivation';

afterEach(() => vi.useRealTimers());

it('waits for the accepted revision instead of treating submission as activation', async () => {
  vi.useFakeTimers();
  const getGraphStatus = vi
    .fn()
    .mockResolvedValueOnce({ activeRevision: 4 })
    .mockResolvedValue({ activeRevision: 5 });
  const port = { getGraphStatus };
  const result = waitForGraphActivation(port, 5, () => true, timerScheduler, monotonicClock);
  await vi.advanceTimersByTimeAsync(25);
  await expect(result).resolves.toBe(true);
  expect(getGraphStatus).toHaveBeenCalledTimes(2);
  expect(vi.getTimerCount()).toBe(0);
});

it('stops on cancellation after a pending read without another native request', async () => {
  let resolve!: (value: unknown) => void;
  let current = true;
  const getGraphStatus = vi.fn().mockReturnValue(
    new Promise((done) => {
      resolve = done;
    }),
  );
  const result = waitForGraphActivation(
    { getGraphStatus },
    5,
    () => current,
    timerScheduler,
    monotonicClock,
  );
  current = false;
  resolve({ activeRevision: 4 });
  await expect(result).resolves.toBe(false);
  expect(getGraphStatus).toHaveBeenCalledTimes(1);
});

it('bounds activation polling and leaves no timer after the deadline', async () => {
  vi.useFakeTimers();
  const getGraphStatus = vi.fn().mockResolvedValue({ activeRevision: 4 });
  const result = waitForGraphActivation(
    { getGraphStatus },
    5,
    () => true,
    timerScheduler,
    monotonicClock,
  ).catch((error: unknown) => error);
  await vi.advanceTimersByTimeAsync(3000);
  expect(await result).toEqual(new Error('Graph activation timed out'));
  expect(getGraphStatus).toHaveBeenCalledTimes(120);
  expect(vi.getTimerCount()).toBe(0);
});

it('does not read graph status when an activation request is already cancelled', async () => {
  const getGraphStatus = vi.fn();
  await expect(
    waitForGraphActivation({ getGraphStatus }, 5, () => false, timerScheduler, monotonicClock),
  ).resolves.toBe(false);
  expect(getGraphStatus).not.toHaveBeenCalled();
});

it('cancels at the activation deadline without showing a timeout for a replaced graph', async () => {
  vi.useFakeTimers();
  let current = true;
  const getGraphStatus = vi.fn().mockResolvedValue({ activeRevision: 4 });
  const result = waitForGraphActivation(
    { getGraphStatus },
    5,
    () => current,
    timerScheduler,
    monotonicClock,
  );
  setTimeout(() => {
    current = false;
  }, 3000);
  await vi.advanceTimersByTimeAsync(3000);
  await expect(result).resolves.toBe(false);
  expect(getGraphStatus).toHaveBeenCalledTimes(120);
  expect(vi.getTimerCount()).toBe(0);
});
