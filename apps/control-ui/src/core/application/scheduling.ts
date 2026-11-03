import type { Cancel, Scheduler } from '../ports/Scheduler';

export function repeat(scheduler: Scheduler, delayMs: number, task: () => void): Cancel {
  let cancelled = false;
  let cancel: Cancel;
  const run = () => {
    task();
    if (!cancelled) cancel = scheduler.after(delayMs, run);
  };
  cancel = scheduler.after(delayMs, run);
  return () => {
    cancelled = true;
    cancel();
  };
}
