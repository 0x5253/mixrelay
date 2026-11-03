import { useEffect, useMemo, useSyncExternalStore } from 'react';
import { timerScheduler } from '../../core/adapters/browser/timers';
import type { Scheduler } from '../../core/ports/Scheduler';
import type { UpdatesPort } from '../../core/ports/UpdatesPort';
import { UpdatesSession } from './application/UpdatesSession';

export function useUpdatesSession(port: UpdatesPort | null, scheduler: Scheduler = timerScheduler) {
  const session = useMemo(() => new UpdatesSession(port, scheduler), [port, scheduler]);
  const state = useSyncExternalStore(session.subscribe, session.snapshot, session.snapshot);
  useEffect(() => {
    session.start();
    return session.dispose;
  }, [session]);
  return { ...state, port, refresh: session.refresh, openDownload: session.openDownload };
}
