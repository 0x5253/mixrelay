import { act, renderHook } from '@testing-library/react';
import { afterEach, assert, beforeEach, expect, it, vi } from 'vitest';
import type { DeviceAppActivity } from '../../core/domain/control/inventory';
import { useVirtualDeviceAppActivity } from './useVirtualDeviceAppActivity';
import { virtualPolicy } from './virtualEndpointFixtures';

const devices = {
  native: [{ bundleId: 'app', displayName: 'App', processId: 1, playing: true, recording: false }],
};
const portWith = (read = vi.fn().mockResolvedValue(devices)) => ({
  getVirtualDeviceAppActivity: read,
});
const tick = (ms = 0) =>
  act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
function pending() {
  let resolve!: (value: Record<string, DeviceAppActivity[]>) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<Record<string, DeviceAppActivity[]>>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}
function visibility(state: 'hidden' | 'visible') {
  Object.defineProperty(document, 'visibilityState', { configurable: true, value: state });
  act(() => document.dispatchEvent(new Event('visibilitychange')));
}
beforeEach(() => {
  vi.useFakeTimers();
  visibility('visible');
});
afterEach(() => {
  vi.useRealTimers();
});

it('distinguishes unsupported, loading, successful empty, rejection and recovery', async () => {
  const read = vi.fn().mockResolvedValue({});
  const port = portWith(read);
  const hook = renderHook(({ port }) => useVirtualDeviceAppActivity(port), {
    initialProps: { port: {} },
  });
  expect(hook.result.current).toEqual({ state: 'unavailable' });
  hook.rerender({ port });
  expect(hook.result.current).toEqual({ state: 'loading' });
  await tick();
  expect(hook.result.current).toEqual({ state: 'observed', devices: {} });
  read.mockRejectedValueOnce(new Error('Linux'));
  await tick(1000);
  expect(hook.result.current).toEqual({ state: 'unavailable' });
  await tick(1000);
  expect(hook.result.current).toEqual({ state: 'observed', devices: {} });
  hook.rerender({ port: {} });
  expect(hook.result.current).toEqual({ state: 'unavailable' });
});

it('polls one second after completion, shares unchanged results and expires hung observations at three seconds', async () => {
  const slow = pending();
  const read = vi.fn().mockResolvedValue(devices);
  const port = portWith(read);
  const hook = renderHook(() => useVirtualDeviceAppActivity(port));
  await tick();
  const first = hook.result.current;
  await tick(999);
  expect(read).toHaveBeenCalledTimes(1);
  await tick(1);
  expect(hook.result.current).toBe(first);
  read.mockReturnValueOnce(slow.promise);
  await tick(1000);
  act(() => window.dispatchEvent(new Event('focus')));
  await tick(1999);
  expect(hook.result.current).toBe(first);
  await tick(1);
  expect(hook.result.current).toEqual({ state: 'unavailable' });
  expect(read).toHaveBeenCalledTimes(3);
  await act(async () => slow.resolve(devices));
  expect(hook.result.current).toEqual(first);
  await tick(999);
  expect(read).toHaveBeenCalledTimes(3);
  await tick(1);
  expect(read).toHaveBeenCalledTimes(4);
  hook.unmount();
  await tick(5000);
  expect(read).toHaveBeenCalledTimes(4);
});

it('refreshes on focus and visibility, retires hidden replies, and ignores late success/failure after replacement', async () => {
  const oldSuccess = pending();
  const oldFailure = pending();
  const hidden = pending();
  const read = vi.fn().mockReturnValueOnce(hidden.promise).mockResolvedValue(devices);
  const port = portWith(read);
  const hook = renderHook(({ port, settings }) => useVirtualDeviceAppActivity(port, settings), {
    initialProps: { port, settings: virtualPolicy },
  });
  visibility('hidden');
  await act(async () => hidden.resolve(devices));
  await tick(4000);
  expect(read).toHaveBeenCalledTimes(1);
  expect(hook.result.current).toEqual({ state: 'unavailable' });
  act(() => window.dispatchEvent(new Event('focus')));
  expect(read).toHaveBeenCalledTimes(1);
  visibility('visible');
  await tick();
  expect(read).toHaveBeenCalledTimes(2);
  act(() => window.dispatchEvent(new Event('focus')));
  await tick();
  expect(read).toHaveBeenCalledTimes(3);
  read.mockReturnValueOnce(oldSuccess.promise);
  await tick(1000);
  const [device] = virtualPolicy.devices;
  assert(device);
  const changed = { ...virtualPolicy, devices: [{ ...device, nativeDeviceIds: ['replacement'] }] };
  hook.rerender({ port, settings: changed });
  expect(hook.result.current).toEqual({ state: 'loading' });
  await tick();
  await act(async () => oldSuccess.resolve({ wrong: [] }));
  expect(hook.result.current).toEqual({ state: 'observed', devices });
  read.mockReturnValueOnce(oldFailure.promise);
  await tick(1000);
  const replacement = portWith();
  hook.rerender({ port: replacement, settings: changed });
  await tick();
  await act(async () => oldFailure.reject(new Error('retired')));
  expect(hook.result.current).toEqual({ state: 'observed', devices });
});

it('expires an initial hung read and does not start while initially hidden', async () => {
  visibility('hidden');
  const slow = pending();
  const read = vi.fn().mockReturnValue(slow.promise);
  const port = portWith(read);
  const hook = renderHook(() => useVirtualDeviceAppActivity(port));
  expect(read).not.toHaveBeenCalled();
  await tick(3000);
  expect(hook.result.current).toEqual({ state: 'unavailable' });
  visibility('visible');
  expect(read).toHaveBeenCalledTimes(1);
  await tick(3000);
  expect(hook.result.current).toEqual({ state: 'unavailable' });
  hook.unmount();
  await act(async () => slow.resolve(devices));
  await tick(5000);
  expect(read).toHaveBeenCalledTimes(1);
});
