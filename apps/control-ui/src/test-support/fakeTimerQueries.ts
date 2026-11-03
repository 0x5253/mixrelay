import { act, cleanup, screen, waitFor } from '@testing-library/react';
import { vi } from 'vitest';

/** Under fake timers, runs timers due within `ms` (0 settles pending promises and
 * zero-delay work); under real timers it only flushes microtasks. */
export async function advance(ms = 0) {
  if (vi.isFakeTimers()) await act(async () => vi.advanceTimersByTimeAsync(ms));
  else await act(async () => Promise.resolve());
}

/** Test teardown for fake-timer tests: drains timers, then always unmounts and restores
 * real timers and mocks, even when draining rejects. Vitest skips later afterEach hooks
 * after one throws, so the global cleanup cannot be relied on here. */
export async function finishFakeTimerTest(
  drain: () => Promise<unknown> = advance,
  restore: () => void = () => {},
) {
  try {
    await drain();
  } finally {
    try {
      cleanup();
    } finally {
      vi.clearAllTimers();
      vi.useRealTimers();
      vi.restoreAllMocks();
      vi.unstubAllGlobals();
      restore();
    }
  }
}

// Captured at load, before any test fakes timers, so these keep reading the real clock.
const realNow = performance.now.bind(performance);
const realSetTimeout = globalThis.setTimeout.bind(globalThis);
const realWaitStep = 20;

/** These helpers step the fake clock 250 ms at a time until the
 * query or assertion holds — like waitFor, but without wall-clock waits such as the
 * one-second node-control readback. Once `options.timeout` of fake time is used up
 * (2000 ms by default) they keep checking on the real clock for `options.realTimeout`
 * (2000 ms by default, measured from that point), so real asynchronous work that fake
 * timers do not control can finish on a loaded machine. The real budget stays below
 * every test timeout, so a genuine failure rejects here instead of outliving its test.
 * Under real timers they defer to waitFor/findByRole with the same timeout. */
const fakeStep = 250;

async function waitReal() {
  await act(() => new Promise<void>((resolve) => realSetTimeout(resolve, realWaitStep)));
  await advance();
}

export interface EventuallyOptions {
  /** Fake time under fake timers; the waitFor timeout under real timers. */
  timeout?: number;
  /** Real time allowed after the fake time is used up (fake timers only). */
  realTimeout?: number;
}

export async function eventually<T>(assertion: () => T, options: EventuallyOptions = {}) {
  const { timeout, realTimeout = 2000 } = options;
  if (!vi.isFakeTimers()) return waitFor(assertion, timeout === undefined ? {} : { timeout });
  const limit = timeout ?? 2000;
  await advance();
  let fakeElapsed = 0;
  let realStart: number | undefined;
  for (;;) {
    try {
      return assertion();
    } catch (error) {
      if (fakeElapsed < limit) {
        fakeElapsed += fakeStep;
        await advance(fakeStep);
        continue;
      }
      realStart ??= realNow();
      if (realNow() - realStart >= realTimeout) throw error;
      await waitReal();
    }
  }
}

export type RoleScope = Pick<typeof screen, 'findByRole' | 'getByRole'>;
type RoleQuery = Parameters<typeof screen.getByRole>;

export async function findIn(
  scope: RoleScope,
  role: RoleQuery[0],
  options?: RoleQuery[1],
  wait: EventuallyOptions = {},
) {
  if (!vi.isFakeTimers())
    return scope.findByRole(
      role,
      options,
      wait.timeout === undefined ? {} : { timeout: wait.timeout },
    );
  return eventually(() => scope.getByRole(role, options), wait);
}

export function find(role: RoleQuery[0], options?: RoleQuery[1], wait?: EventuallyOptions) {
  return findIn(screen, role, options, wait);
}
