import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DriverSnapshot } from '../../core/domain/driver';
import { i18n } from '../../core/i18n';
import type { DriverPort } from '../../core/ports/DriverPort';
import { readyDriver } from './driverFixtures';
import { useDriverSession } from './useDriverSession';

const device = readyDriver.settings.devices[0];
if (!device) throw new Error('Driver fixture needs a device');
function port(snapshot = readyDriver): DriverPort {
  return {
    getSnapshot: vi.fn().mockResolvedValue(snapshot),
    mutate: vi.fn().mockResolvedValue({ ...snapshot, revision: snapshot.revision + 1 }),
  };
}
const remove = { action: 'remove', endpointId: 'device-a' } as const;
beforeEach(async () => {
  await i18n.changeLanguage('en');
});
afterEach(() => vi.useRealTimers());

describe('Driver settings readback and optional stream directions', () => {
  it.each([
    [1, 'mono'],
    [2, 'stereo'],
    [4, 'discrete'],
  ] as const)('updates both directions to %i channels with %s layout', async (channels, layout) => {
    const snapshot: DriverSnapshot = {
      ...readyDriver,
      settings: {
        ...readyDriver.settings,
        devices: [
          {
            ...device,
            spec: {
              ...device.spec,
              render: { channels: 8, channelLayout: 'discrete' },
              capture: { channels: 8, channelLayout: 'discrete' },
            },
          },
        ],
      },
    };
    const native = port(snapshot);
    const { result } = renderHook(() => useDriverSession(native));
    await act(async () => Promise.resolve());
    await act(async () =>
      result.current.changeDevice('device-a', {
        displayName: 'Monitor',
        renderChannels: channels,
        captureChannels: channels,
      }),
    );
    expect(native.mutate).toHaveBeenCalledExactlyOnceWith({
      action: 'configure',
      expectedRevision: 7,
      spec: {
        ...device.spec,
        render: { channels, channelLayout: layout },
        capture: { channels, channelLayout: layout },
      },
    });
  });

  it.each(['render', 'capture'] as const)(
    'never invents a missing %s direction',
    async (direction) => {
      const spec = { ...device.spec, [direction]: undefined };
      const native = port({
        ...readyDriver,
        settings: { ...readyDriver.settings, devices: [{ ...device, spec }] },
      });
      const { result } = renderHook(() => useDriverSession(native));
      await act(async () => Promise.resolve());
      await act(async () =>
        result.current.changeDevice('device-a', {
          displayName: 'Monitor',
          renderChannels: 2,
          captureChannels: 2,
        }),
      );
      expect(native.mutate).toHaveBeenCalledExactlyOnceWith({
        action: 'configure',
        expectedRevision: 7,
        spec,
      });
    },
  );

  it('keeps accepted settings visible while explaining a persistence failure', async () => {
    const native = port();
    vi.mocked(native.mutate).mockResolvedValue({
      ...readyDriver,
      revision: 8,
      persistenceError: 'disk full',
    });
    const { result } = renderHook(() => useDriverSession(native));
    await act(async () => Promise.resolve());
    await act(async () => result.current.mutate(remove));
    expect(result.current.snapshot?.revision).toBe(8);
    expect(result.current.error).toMatch(/save/i);
    expect(result.current.busy).toBe(false);
  });
});

describe('Driver mutation preconditions', () => {
  it('keeps a missing Driver inert and resets stale state when its port disappears', async () => {
    const native = port();
    const { result, rerender } = renderHook(
      ({ value }: { value: DriverPort | null }) => useDriverSession(value),
      {
        initialProps: { value: native as DriverPort | null },
      },
    );
    await act(async () => Promise.resolve());
    rerender({ value: null });
    await act(async () => result.current.refresh());
    await act(async () => expect(result.current.mutate(remove)).resolves.toBeNull());
    expect(result.current.snapshot).toBeNull();
    expect(result.current.busy).toBe(false);
    expect(result.current.error).toBeNull();
    expect(native.mutate).not.toHaveBeenCalled();
  });

  it.each(['not_installed', 'installed_not_loaded', 'unavailable'] as const)(
    'refuses to send settings to a %s Driver',
    async (state) => {
      const native = port({ ...readyDriver, state });
      const { result } = renderHook(() => useDriverSession(native));
      await act(async () => Promise.resolve());
      await act(async () => expect(result.current.mutate(remove)).resolves.toBeNull());
      expect(native.mutate).not.toHaveBeenCalled();
    },
  );

  it('allows creation with a new identity but refuses a removed target', async () => {
    const native = port();
    const { result } = renderHook(() => useDriverSession(native));
    await act(async () => Promise.resolve());
    const creation = { action: 'create', spec: { ...device.spec, id: 'new-device' } } as const;
    await act(async () => result.current.mutate(creation));
    expect(native.mutate).toHaveBeenCalledExactlyOnceWith({ ...creation, expectedRevision: 7 });
    await act(async () => result.current.mutate({ action: 'remove', endpointId: 'gone' }));
    expect(native.mutate).toHaveBeenCalledTimes(1);
    expect(result.current.endpointId).toBe('gone');
    expect(result.current.error).toMatch(/no longer exists/);
  });
});

describe('Driver polling and request lifetime', () => {
  it('does not overlap a pending health read and leaves device errors attached through polling', async () => {
    vi.useFakeTimers();
    const native = port();
    const { result, unmount } = renderHook(() => useDriverSession(native));
    await act(async () => Promise.resolve());
    await act(async () => result.current.mutate({ action: 'remove', endpointId: 'gone' }));
    const missing = result.current.error;
    const pending = Promise.withResolvers<DriverSnapshot>();
    vi.mocked(native.getSnapshot).mockReturnValueOnce(pending.promise);
    let request!: Promise<void>;
    act(() => {
      request = result.current.refresh();
    });
    await act(async () => result.current.refresh());
    expect(native.getSnapshot).toHaveBeenCalledTimes(2);
    await act(async () => {
      pending.resolve(readyDriver);
      await request;
    });
    expect(result.current.error).toBe(missing);
    await act(async () => vi.advanceTimersByTimeAsync(5_000));
    expect(result.current.error).toBe(missing);
    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });

  it.each(['read', 'write'] as const)(
    'ignores a late %s rejection after unmount',
    async (operation) => {
      const native = port();
      const pending = Promise.withResolvers<DriverSnapshot>();
      if (operation === 'read') vi.mocked(native.getSnapshot).mockReturnValue(pending.promise);
      const { result, unmount } = renderHook(() => useDriverSession(native));
      await act(async () => Promise.resolve());
      let request: Promise<DriverSnapshot | null> | undefined;
      if (operation === 'write') {
        vi.mocked(native.mutate).mockReturnValueOnce(pending.promise);
        act(() => {
          request = result.current.mutate(remove);
        });
      }
      const state = result.current;
      unmount();
      await act(async () => {
        pending.reject(new Error('late request failure'));
        await request;
      });
      expect(result.current).toBe(state);
    },
  );

  it('cancels startup work before its deferred first poll when unmounted immediately', async () => {
    const native = port();
    const { unmount } = renderHook(() => useDriverSession(native));
    unmount();
    await act(async () => Promise.resolve());
    expect(native.getSnapshot).not.toHaveBeenCalled();
  });
});
