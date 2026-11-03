import { useCallback, useEffect, useMemo, useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';
import { timerScheduler } from '../../core/adapters/browser/timers';
import type { DriverAction } from '../../core/domain/driver';
import type { DriverPort } from '../../core/ports/DriverPort';
import type { Scheduler } from '../../core/ports/Scheduler';
import { DriverSession as DriverSessionOwner } from './application/DriverSession';
import type { EndpointConfiguration } from './domain/deviceControl';

export function useDriverSession(port: DriverPort | null, scheduler: Scheduler = timerScheduler) {
  const { t } = useTranslation();
  const session = useMemo(() => new DriverSessionOwner(port, scheduler), [port, scheduler]);
  const state = useSyncExternalStore(session.subscribe, session.snapshot, session.snapshot);
  useEffect(() => {
    session.start();
    return session.dispose;
  }, [session]);
  const mutate = useCallback((action: DriverAction) => session.mutate(action), [session]);
  const changeDevice = useCallback(
    (id: string, update: EndpointConfiguration) => session.changeDevice(id, update),
    [session],
  );
  return {
    ...state,
    port,
    error: state.error ? t(`driverManagement.${state.error}`) : null,
    refresh: session.refresh,
    mutate,
    changeDevice,
  };
}

export type DriverSession = ReturnType<typeof useDriverSession>;
