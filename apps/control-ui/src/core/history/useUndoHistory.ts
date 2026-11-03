import { useEffect, useState } from 'react';
import { systemClock, timerScheduler } from '../adapters/browser/timers';
import { UndoHistory } from '../application/history/UndoHistory';
import type { Clock, Scheduler } from '../ports/Scheduler';

/** One UndoHistory for the component's lifetime. Its methods keep their identity across
 * renders; unmounting drops a pending gesture instead of committing it in the background. */
export function useUndoHistory<Value, Kind>(
  clock: Clock = systemClock,
  scheduler: Scheduler = timerScheduler,
) {
  const [history] = useState(() => new UndoHistory<Value, Kind>(clock, scheduler));
  const [methods] = useState(() => ({
    clear: () => history.clear(),
    flush: () => history.flush(),
    record: (before: Value, after: Value, kind: Kind, group?: string) =>
      history.record(before, after, kind, group),
    recordDebounced: (before: Value, after: Value, kind: Kind, group: string, delayMs?: number) =>
      history.recordDebounced(before, after, kind, group, delayMs),
    redo: () => history.redo(),
    undo: () => history.undo(),
    rebase: (transform: (value: Value) => Value) => history.rebase(transform),
  }));
  useEffect(() => () => history.dispose(), [history]);
  return methods;
}
