import { useEffect, useMemo, useSyncExternalStore } from 'react';
import { timerScheduler } from '../../core/adapters/browser/timers';
import type { RuntimePort } from '../../core/ports/RuntimePort';
import type { Scheduler } from '../../core/ports/Scheduler';
import { RuntimeConnection } from './application/RuntimeConnection';

export function useRuntimeConnection(port: RuntimePort, scheduler: Scheduler = timerScheduler) {
  const connection = useMemo(() => new RuntimeConnection(scheduler), [scheduler]);
  const state = useSyncExternalStore(
    connection.subscribe,
    connection.snapshot,
    connection.snapshot,
  );
  useEffect(() => {
    connection.start(port);
    return connection.dispose;
  }, [connection, port]);
  return { ...state, connect: connection.connect };
}
