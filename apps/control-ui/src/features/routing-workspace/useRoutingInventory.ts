import { useCallback, useEffect, useRef } from 'react';
import { timerScheduler } from '../../core/adapters/browser/timers';
import { repeat } from '../../core/application/scheduling';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import type { Scheduler } from '../../core/ports/Scheduler';
import { useSharedState } from '../../core/ui/useStructuralShare';

export const INVENTORY_POLL_MS = 5_000;
export const DEVICE_PRESENCE_POLL_MS = 2_000;

/** Device presence stays current while visible; native app/icon discovery only polls in menus. */
export function useRoutingInventory(
  controlPort: Pick<ControlPort, 'listCapturableApplications' | 'listAudioDevices'>,
  menuOpened: boolean,
  scheduler: Scheduler = timerScheduler,
) {
  const loadApplications = useCallback(
    () => controlPort.listCapturableApplications?.() ?? Promise.resolve([]),
    [controlPort],
  );
  const loadDevices = useCallback(
    () => controlPort.listAudioDevices?.() ?? Promise.resolve([]),
    [controlPort],
  );
  const applications = useInventoryCategory(loadApplications);
  const devices = useInventoryCategory(loadDevices);
  const refreshApplications = applications.refresh;
  const refreshDevices = devices.refresh;
  const pollDevices = devices.poll;
  const refresh = useCallback(() => {
    refreshApplications();
    refreshDevices();
  }, [refreshApplications, refreshDevices]);

  useEffect(() => {
    const timer = repeat(scheduler, DEVICE_PRESENCE_POLL_MS, pollDevices);
    window.addEventListener('focus', refresh);
    document.addEventListener('visibilitychange', refresh);
    return () => {
      timer?.();
      window.removeEventListener('focus', refresh);
      document.removeEventListener('visibilitychange', refresh);
    };
  }, [refresh, pollDevices, scheduler]);

  useEffect(() => {
    if (!menuOpened) return;
    refresh();
    const timer = repeat(scheduler, INVENTORY_POLL_MS, refresh);
    return () => timer?.();
  }, [menuOpened, refresh, scheduler]);

  return {
    applications: applications.items,
    audioDevices: devices.items,
    devicesObserved: Boolean(controlPort.listAudioDevices) && devices.observed,
    failed: applications.failed || devices.failed,
    refreshing: applications.refreshing || devices.refreshing,
    refresh,
  };
}

interface InventorySnapshot<T> {
  items: T[];
  observed: boolean;
  failed: boolean;
  refreshing: boolean;
}

/** Each category owns one request fence: a hung icon lookup cannot block device presence.
 * Native invokes cannot be cancelled, so cleanup invalidates their publication instead. */
function useInventoryCategory<T>(load: () => Promise<T[]>) {
  const [snapshot, setSnapshot] = useSharedState<InventorySnapshot<T>>({
    items: [],
    observed: false,
    failed: false,
    refreshing: false,
  });
  const request = useRef<((quiet?: boolean) => void) | null>(null);
  const refresh = useCallback(() => request.current?.(), []);
  const poll = useCallback(() => request.current?.(true), []);

  useEffect(() => {
    let active = true;
    let running = false;
    setSnapshot({ items: [], observed: false, failed: false, refreshing: false });
    const finish = (outcome: { items: T[] } | { failed: true }) => {
      running = false;
      if (!active) return;
      setSnapshot((current) => {
        if (document.visibilityState === 'hidden') return { ...current, refreshing: false };
        return 'items' in outcome
          ? { items: outcome.items, observed: true, failed: false, refreshing: false }
          : { ...current, failed: true, refreshing: false };
      });
    };
    // Background presence polls stay quiet: only a requested refresh shows progress.
    const discover = (quiet = false) => {
      if (!active || running || document.visibilityState === 'hidden') return;
      running = true;
      if (!quiet) setSnapshot((current) => ({ ...current, refreshing: true }));
      void Promise.resolve()
        .then(load)
        .then(
          (items) => finish({ items }),
          () => finish({ failed: true }),
        );
    };
    request.current = discover;
    discover();
    return () => {
      active = false;
      request.current = null;
    };
  }, [load, setSnapshot]);

  return { ...snapshot, refresh, poll };
}
