import { expect, it, vi } from 'vitest';
import type { DriverCommand } from '../../domain/driver';
import { TauriDriverPort } from './TauriDriverPort';

const snapshot = {
  protocolVersion: 1,
  revision: 7,
  state: 'ready',
  installedVersion: '1.0.0',
  ownerProcessId: 12,
  settings: {
    canCreate: true,
    canRename: true,
    renderChannels: [2],
    captureChannels: [2],
    creationTemplate: null,
    devices: [],
  },
};

function port() {
  const invoke = vi.fn().mockResolvedValue(snapshot);
  return { invoke, port: new TauriDriverPort(invoke) };
}

it('uses only Driver owner commands and full directional formats', async () => {
  const { invoke, port: driver } = port();
  const command = {
    action: 'set_format' as const,
    endpointId: 'device-a',
    expectedRevision: 7,
    direction: 'capture' as const,
    format: { sampleRate: 48000, channels: 2, sampleFormat: 'signed32' as const },
  };
  await expect(driver.getSnapshot()).resolves.toEqual(snapshot);
  await driver.mutate(command);
  expect(invoke.mock.calls).toEqual([['driver_status'], ['driver_mutate', { command }]]);
});

it('sends the device sample format without directional format settings', async () => {
  const { invoke, port: driver } = port();
  const command: DriverCommand = {
    action: 'set_sample_format',
    endpointId: 'device-a',
    expectedRevision: 7,
    sampleFormat: 'signed24',
  };
  await driver.mutate(command);
  expect(invoke).toHaveBeenCalledExactlyOnceWith('driver_mutate', { command });
});

it('refuses a snapshot from another protocol or with a malformed device', async () => {
  const { invoke, port: driver } = port();
  invoke.mockResolvedValueOnce({ ...snapshot, protocolVersion: 2 });
  await expect(driver.getSnapshot()).rejects.toThrow('driver_status.protocolVersion');
  const device = { nativeDeviceIds: [], spec: {}, observedSampleRate: null };
  invoke.mockResolvedValueOnce({
    ...snapshot,
    settings: { ...snapshot.settings, devices: [device] },
  });
  await expect(driver.getSnapshot()).rejects.toThrow('driver_status.settings.devices[0]');
});

it('propagates Driver owner failures unchanged', async () => {
  const { invoke, port: driver } = port();
  const failure = new Error('Driver is unavailable');
  invoke.mockRejectedValueOnce(failure);
  await expect(driver.getSnapshot()).rejects.toBe(failure);
});
