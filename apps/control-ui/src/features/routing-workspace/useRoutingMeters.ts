import { useEffect, useMemo } from 'react';
import { monotonicClock, timerScheduler } from '../../core/adapters/browser/timers';
import { AudioMeterStore } from '../../core/audio-meter/AudioMeterStore';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import type { MonotonicClock, Scheduler } from '../../core/ports/Scheduler';
import { MeterRequestOwner, MeterSession } from './application/MeterSession';

type MeterPort = Pick<ControlPort, 'getMeters' | 'subscribeMeters'>;

/** Each engine owner has an independent store; old replies cannot light up its replacement. */
export function useRoutingMeters(
  controlPort: MeterPort,
  scheduler: Scheduler = timerScheduler,
  clock: MonotonicClock = monotonicClock,
): AudioMeterStore {
  const owner = useMemo(
    () => ({ requests: new MeterRequestOwner(controlPort, clock), store: new AudioMeterStore() }),
    [controlPort, clock],
  );
  useEffect(() => {
    const session = new MeterSession(controlPort, owner.requests, owner.store, scheduler, clock);
    const refresh = () => session.setVisible(document.visibilityState === 'visible');
    document.addEventListener('visibilitychange', refresh);
    refresh();
    return () => {
      document.removeEventListener('visibilitychange', refresh);
      session.dispose();
    };
  }, [controlPort, owner, scheduler, clock]);
  return owner.store;
}
