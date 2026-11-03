import type { Clock, Scheduler } from '../core/ports/Scheduler';

interface Task {
  at: number;
  order: number;
  run: () => void;
}

/** A manual clock and scheduler: nothing runs until the test advances time. Tasks due at
 * the same time run in the order they were scheduled. */
export function fakeScheduler(): Scheduler &
  Clock & { advance(ms: number): void; pending(): number } {
  let now = 0;
  let order = 0;
  const tasks = new Set<Task>();
  const next = (until: number) =>
    [...tasks]
      .filter((task) => task.at <= until)
      .sort((a, b) => a.at - b.at || a.order - b.order)[0];
  return {
    now: () => now,
    pending: () => tasks.size,
    after(delayMs, run) {
      const task = { at: now + Math.max(0, delayMs), order: order++, run };
      tasks.add(task);
      return () => tasks.delete(task);
    },
    advance(ms) {
      const until = now + ms;
      for (let task = next(until); task; task = next(until)) {
        tasks.delete(task);
        now = task.at;
        task.run();
      }
      now = until;
    },
  };
}
