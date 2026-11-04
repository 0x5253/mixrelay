import type { VirtualEndpointSpec } from '../core/domain/control/status';
import type { DriverSession } from '../features/driver-status/useDriverSession';
import type { VirtualEndpointUpdate } from '../features/routing-workspace/virtualEndpointNodes';

/** The Driver session as the routing workspace's virtual-device editor sees it. */
export function driverRoutingActions(session: DriverSession | undefined) {
  return {
    settings: session?.snapshot?.settings,
    enabled: session?.snapshot?.state === 'ready' && !session.busy,
    error: session?.error ? { endpointId: session.endpointId, message: session.error } : null,
    change: (endpointId: string, update: VirtualEndpointUpdate) => {
      void session?.changeDevice(endpointId, update);
    },
    /** The created device as the Driver confirmed it, or null without a confirmed match. */
    create: async (spec: VirtualEndpointSpec) => {
      const snapshot = await session?.mutate({ action: 'create', spec });
      return snapshot?.settings.devices.find((device) => device.spec.id === spec.id)?.spec ?? null;
    },
  };
}
