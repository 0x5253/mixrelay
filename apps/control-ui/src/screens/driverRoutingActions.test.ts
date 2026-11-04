import { expect, it, vi } from 'vitest';
import type { VirtualEndpointSpec } from '../core/domain/control/status';
import type { DriverSession } from '../features/driver-status/useDriverSession';
import { virtualPolicy, virtualSpec } from '../features/routing-workspace/virtualEndpointFixtures';
import { driverRoutingActions } from './driverRoutingActions';

function session(overrides: Partial<DriverSession> = {}): DriverSession {
  return {
    port: null,
    snapshot: {
      protocolVersion: 1,
      revision: 1,
      state: 'ready',
      installedVersion: '0.1.0',
      settings: virtualPolicy,
    },
    busy: false,
    error: null,
    endpointId: null,
    refresh: vi.fn().mockResolvedValue(undefined),
    changeDevice: vi.fn().mockResolvedValue(null),
    mutate: vi.fn().mockResolvedValue(null),
    ...overrides,
  };
}

it('enables device edits only for a ready, idle Driver and keeps its error with the endpoint', () => {
  const ready = driverRoutingActions(session());
  expect(ready).toMatchObject({ settings: virtualPolicy, enabled: true, error: null });
  expect(driverRoutingActions(session({ busy: true })).enabled).toBe(false);
  expect(
    driverRoutingActions(session({ snapshot: null, error: 'Driver unavailable', endpointId: 'a' })),
  ).toMatchObject({
    settings: undefined,
    enabled: false,
    error: { endpointId: 'a', message: 'Driver unavailable' },
  });
  expect(driverRoutingActions(undefined)).toMatchObject({ enabled: false, error: null });
});

it('sends device changes to the Driver owner', () => {
  const driver = session();
  const update = {
    displayName: 'Monitor',
    renderChannels: 2,
    captureChannels: 2,
    sampleFormat: 'signed24' as const,
  };
  driverRoutingActions(driver).change('device-a', update);
  driverRoutingActions(undefined).change('device-a', update);
  expect(driver.changeDevice).toHaveBeenCalledExactlyOnceWith('device-a', update);
});

it('returns the created device as the Driver confirmed it, or null without a confirmed match', async () => {
  const driver = session();
  const spec: VirtualEndpointSpec = { ...virtualSpec, id: 'new-device' };
  vi.mocked(driver.mutate).mockResolvedValueOnce({
    protocolVersion: 1,
    revision: 2,
    state: 'ready',
    installedVersion: '0.1.0',
    settings: {
      ...virtualPolicy,
      devices: virtualPolicy.devices.map((device) => ({ ...device, spec })),
    },
  });
  const actions = driverRoutingActions(driver);
  expect(await actions.create(spec)).toEqual(spec);
  expect(driver.mutate).toHaveBeenCalledExactlyOnceWith({ action: 'create', spec });
  expect(await actions.create(spec)).toBeNull();
  expect(await driverRoutingActions(undefined).create(spec)).toBeNull();
});
