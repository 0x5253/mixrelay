import { useEffect, useMemo, useState } from 'react';
import { timerScheduler } from '../../core/adapters/browser/timers';
import type { MidiPort } from '../../core/ports/MidiPort';
import type { Scheduler } from '../../core/ports/Scheduler';
import { MidiSession, type MidiState } from './application/MidiSession';

const emptyState: MidiState = { status: null, busy: false, error: false };

export function useMidiSession(
  port: MidiPort,
  opened: boolean,
  scheduler: Scheduler = timerScheduler,
) {
  const [observed, setObserved] = useState({ port, state: emptyState });
  const session = useMemo(
    () => new MidiSession(port, scheduler, (state) => setObserved({ port, state })),
    [port, scheduler],
  );
  useEffect(() => {
    session.start(opened);
    return session.dispose;
  }, [session, opened]);
  return { ...(observed.port === port ? observed.state : emptyState), command: session.command };
}
