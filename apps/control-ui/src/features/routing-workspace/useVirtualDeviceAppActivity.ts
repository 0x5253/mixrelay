import { useEffect, useState } from 'react';
import { timerScheduler } from '../../core/adapters/browser/timers';
import type { VirtualEndpointSettings } from '../../core/domain/control/status';
import { structuralShare } from '../../core/domain/structuralShare';
import type { InventoryPort } from '../../core/ports/control/InventoryPort';
import type { Scheduler } from '../../core/ports/Scheduler';
import {
  type DeviceActivityObservation,
  DeviceActivitySession,
} from './application/DeviceActivitySession';

type ActivityPort = Pick<InventoryPort, 'getVirtualDeviceAppActivity'>;

/** One workspace reader. Identity fences retire replies across ports, UID mappings and visibility. */
export function useVirtualDeviceAppActivity(
  controlPort: ActivityPort,
  settings?: VirtualEndpointSettings,
  scheduler: Scheduler = timerScheduler,
) {
  const mapping = JSON.stringify(
    settings?.devices.map(({ spec, nativeDeviceIds }) => [spec.id, nativeDeviceIds]),
  );
  const [snapshot, setSnapshot] = useState<{
    port: ActivityPort;
    mapping: string | undefined;
    value: DeviceActivityObservation;
  } | null>(null);
  useEffect(() => {
    const read = controlPort.getVirtualDeviceAppActivity;
    if (!read) return;
    const publish = (value: DeviceActivityObservation) =>
      setSnapshot((previous) => {
        if (previous?.port === controlPort && previous.mapping === mapping) {
          const shared = structuralShare(previous.value, value);
          return shared === previous.value ? previous : { ...previous, value: shared };
        }
        return { port: controlPort, mapping, value };
      });
    const session = new DeviceActivitySession(controlPort, read, publish, scheduler);
    const refresh = () => session.refresh();
    const visibility = () => session.setVisible(document.visibilityState !== 'hidden');
    window.addEventListener('focus', refresh);
    document.addEventListener('visibilitychange', visibility);
    visibility();
    return () => {
      window.removeEventListener('focus', refresh);
      document.removeEventListener('visibilitychange', visibility);
      session.dispose();
    };
  }, [controlPort, mapping, scheduler]);
  if (!controlPort.getVirtualDeviceAppActivity) return { state: 'unavailable' } as const;
  return snapshot?.port === controlPort && snapshot.mapping === mapping
    ? snapshot.value
    : ({ state: 'loading' } as const);
}
