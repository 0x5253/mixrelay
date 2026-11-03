import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { DriverSnapshot } from '../../core/domain/driver';
import { i18n } from '../../core/i18n';
import type { DriverPort } from '../../core/ports/DriverPort';
import { readyDriver } from './driverFixtures';
import { useDriverSession } from './useDriverSession';

const fixtureDevice = readyDriver.settings.devices[0];
if (!fixtureDevice) throw new Error('Driver fixture needs a device.');
function native(): DriverPort {
  return {
    getSnapshot: vi.fn().mockResolvedValue(readyDriver),
    mutate: vi.fn().mockResolvedValue({ ...readyDriver, revision: 8 }),
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
beforeEach(async () => {
  await i18n.changeLanguage('en');
});
afterEach(() => vi.useRealTimers());

it('uses the Driver revision, deduplicates writes and applies returned readback', async () => {
  const port = native();
  const pending = deferred<DriverSnapshot>();
  vi.mocked(port.mutate).mockReturnValueOnce(pending.promise);
  const { result } = renderHook(() => useDriverSession(port));
  await act(async () => Promise.resolve());
  const action = {
    action: 'set_format' as const,
    endpointId: 'device-a',
    direction: 'render' as const,
    format: { sampleRate: 44100, channels: 2, sampleFormat: 'signed24' as const },
  };
  let first!: Promise<DriverSnapshot | null>;
  act(() => {
    first = result.current.mutate(action);
    void result.current.mutate(action);
  });
  expect(port.mutate).toHaveBeenCalledExactlyOnceWith({ ...action, expectedRevision: 7 });
  expect(result.current.snapshot?.revision).toBe(7);
  await act(async () => {
    pending.resolve({ ...readyDriver, revision: 8 });
    await first;
  });
  expect(result.current.snapshot?.revision).toBe(8);
  expect(result.current.busy).toBe(false);
});

it('does not create a missing target while configuring it', async () => {
  const port = native();
  const { result } = renderHook(() => useDriverSession(port));
  await act(async () => Promise.resolve());
  await act(async () =>
    result.current.changeDevice('deleted-device', {
      displayName: 'Old',
      renderChannels: 2,
      captureChannels: 2,
    }),
  );
  expect(port.mutate).not.toHaveBeenCalled();
  expect(result.current.error).toContain('no longer exists');
});

it('keeps an explicit failure and actual fields after a rejected write', async () => {
  const port = native();
  vi.mocked(port.mutate).mockRejectedValue('private native error');
  const { result } = renderHook(() => useDriverSession(port));
  await act(async () => Promise.resolve());
  await act(async () => result.current.mutate({ action: 'remove', endpointId: 'device-a' }));
  expect(result.current.snapshot).toEqual(readyDriver);
  expect(result.current.error).toContain('Refresh and try again');
  expect(result.current.busy).toBe(false);
});

it.each(['read', 'write'])(
  'ignores a late %s completion after a port change',
  async (operation) => {
    const port = native();
    const next = native();
    vi.mocked(next.getSnapshot).mockResolvedValue({ ...readyDriver, revision: 12 });
    const pending = deferred<DriverSnapshot>();
    if (operation === 'read') vi.mocked(port.getSnapshot).mockReturnValue(pending.promise);
    const { result, rerender } = renderHook(({ value }) => useDriverSession(value), {
      initialProps: { value: port },
    });
    await act(async () => Promise.resolve());
    let write: Promise<DriverSnapshot | null> | undefined;
    if (operation === 'write') {
      vi.mocked(port.mutate).mockReturnValue(pending.promise);
      act(() => {
        write = result.current.mutate({ action: 'remove', endpointId: 'device-a' });
      });
    }
    rerender({ value: next });
    await act(async () => Promise.resolve());
    await act(async () => {
      pending.resolve({ ...readyDriver, revision: 99 });
      await write;
    });
    expect(result.current.snapshot?.revision).toBe(12);
    expect(result.current.error).toBeNull();
  },
);

it('allows device removal, creation and renaming using confirmed revisions', async () => {
  const port = native();
  const { result } = renderHook(() => useDriverSession(port));
  await act(async () => Promise.resolve());
  await act(async () => result.current.mutate({ action: 'remove', endpointId: 'device-a' }));
  await act(async () =>
    result.current.mutate({ action: 'create', spec: { ...fixtureDevice.spec, id: 'new-device' } }),
  );
  await act(async () =>
    result.current.changeDevice('device-a', {
      displayName: 'Broadcast',
      renderChannels: 2,
      captureChannels: 2,
    }),
  );
  expect(port.mutate).toHaveBeenNthCalledWith(1, {
    action: 'remove',
    endpointId: 'device-a',
    expectedRevision: 7,
  });
  expect(port.mutate).toHaveBeenNthCalledWith(2, {
    action: 'create',
    spec: { ...fixtureDevice.spec, id: 'new-device' },
    expectedRevision: 8,
  });
  expect(port.mutate).toHaveBeenNthCalledWith(3, {
    action: 'configure',
    spec: { ...fixtureDevice.spec, displayName: 'Broadcast' },
    expectedRevision: 8,
  });
  expect(result.current.error).toBeNull();
});

it('uses Configure for a nominal rate change without inventing native PCM tuples', async () => {
  const port = native();
  const { result } = renderHook(() => useDriverSession(port));
  await act(async () => Promise.resolve());
  await act(async () =>
    result.current.changeDevice('device-a', {
      displayName: 'Monitor',
      renderChannels: 2,
      captureChannels: 2,
      sampleRate: 44100,
    }),
  );
  expect(port.mutate).toHaveBeenCalledExactlyOnceWith({
    action: 'configure',
    expectedRevision: 7,
    spec: { ...fixtureDevice.spec, defaultSampleRate: 44100 },
  });
  expect(result.current.snapshot?.settings.devices[0]?.nativeFormats).toEqual(
    fixtureDevice.nativeFormats,
  );
});

it('refreshes without overlapping reads and clears stale health on failure', async () => {
  vi.useFakeTimers();
  const port = native();
  const { result, unmount } = renderHook(() => useDriverSession(port));
  await act(async () => Promise.resolve());
  vi.mocked(port.getSnapshot).mockRejectedValueOnce('unavailable');
  await act(async () => vi.advanceTimersByTimeAsync(5_000));
  expect(result.current.snapshot).toBeNull();
  expect(result.current.error).toBe('Driver service unavailable.');
  unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it('routes node bit-depth changes through one revision-checked shared Driver command', async () => {
  const port = native();
  const { result } = renderHook(() => useDriverSession(port));
  await act(async () => Promise.resolve());
  await act(async () =>
    result.current.changeDevice('device-a', {
      displayName: 'Monitor',
      renderChannels: 2,
      captureChannels: 2,
      sampleFormat: 'float32',
    }),
  );
  expect(port.mutate).toHaveBeenCalledExactlyOnceWith({
    action: 'set_sample_format',
    endpointId: 'device-a',
    sampleFormat: 'float32',
    expectedRevision: 7,
  });
});
