import type { Clock, MonotonicClock, Scheduler } from '../../ports/Scheduler';

// The globals are read at call time, so fake timers installed by a test still apply.
export const timerScheduler: Scheduler = {
  after(delayMs, task) {
    const timer = setTimeout(task, delayMs);
    return () => clearTimeout(timer);
  },
};

export const systemClock: Clock = { now: () => Date.now() };

export const monotonicClock: MonotonicClock = { now: () => performance.now() };
