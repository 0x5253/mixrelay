import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { RuntimeConnectStage, RuntimeStatus } from '../../core/domain/runtime';
import type { RuntimePort } from '../../core/ports/RuntimePort';
import { useRuntimeConnection } from './useRuntimeConnection';

function port() {
  return {
    connect: vi.fn<RuntimePort['connect']>().mockResolvedValue('ready'),
    status: vi.fn<RuntimePort['status']>().mockResolvedValue('ready'),
  };
}
afterEach(() => vi.useRealTimers());
it.each(['unavailable', 'rejected'] as const)(
  'opens a late-ready runtime after startup is %s without another launch',
  async (outcome) => {
    vi.useFakeTimers();
    const native = port();
    if (outcome === 'rejected') native.connect.mockRejectedValueOnce(new Error('IPC lost'));
    else native.connect.mockResolvedValueOnce('unavailable');
    native.status.mockRejectedValueOnce(new Error('still starting'));
    const { result, unmount } = renderHook(() => useRuntimeConnection(native));
    await act(async () => Promise.resolve());
    expect(result.current).toMatchObject({ state: 'unavailable', opened: false });
    await act(async () => vi.advanceTimersByTimeAsync(2000));
    expect(native.status).toHaveBeenCalledOnce();
    expect(result.current).toMatchObject({ state: 'unavailable', opened: false });
    await act(async () => vi.advanceTimersByTimeAsync(2000));
    expect(result.current).toMatchObject({ state: 'ready', opened: true });
    expect(native.connect).toHaveBeenCalledOnce();
    unmount();
    expect(vi.getTimerCount()).toBe(0);
  },
);

it.each(['not_installed', 'incompatible'] as const)(
  'does not poll or launch again when startup is %s',
  async (state) => {
    vi.useFakeTimers();
    const native = port();
    native.connect.mockResolvedValueOnce(state);
    const { result } = renderHook(() => useRuntimeConnection(native));
    await act(async () => Promise.resolve());
    await act(async () => vi.advanceTimersByTimeAsync(10000));
    expect(result.current).toMatchObject({ state, opened: false });
    expect(native.status).not.toHaveBeenCalled();
    expect(native.connect).toHaveBeenCalledOnce();
  },
);

it('opens only after connecting and observes without restarting the Driver', async () => {
  vi.useFakeTimers();
  const native = port();
  const pending = Promise.withResolvers<RuntimeStatus>();
  native.connect.mockReturnValueOnce(pending.promise);
  const { result, unmount } = renderHook(() => useRuntimeConnection(native));
  expect(result.current).toMatchObject({ state: 'connecting', opened: false });
  await act(async () => pending.resolve('ready'));
  expect(result.current).toMatchObject({ state: 'ready', opened: true });
  await act(async () => vi.advanceTimersByTimeAsync(1999));
  expect(native.status).not.toHaveBeenCalled();
  await act(async () => vi.advanceTimersByTimeAsync(2001));
  expect(native.status).toHaveBeenCalledTimes(2);
  expect(native.connect).toHaveBeenCalledTimes(1);
  unmount();
  await act(async () => vi.advanceTimersByTimeAsync(4000));
  expect(native.status).toHaveBeenCalledTimes(2);
});
it('reports the steps of the current connection only', async () => {
  const native = port();
  let first: (stage: RuntimeConnectStage) => void = () => {};
  native.connect
    .mockImplementationOnce((onStage) => {
      first = onStage;
      return new Promise(() => {});
    })
    .mockReturnValueOnce(new Promise(() => {}));
  const { result } = renderHook(() => useRuntimeConnection(native));
  expect(result.current).toMatchObject({ state: 'connecting', stage: 'checking' });
  act(() => first('starting'));
  expect(result.current.stage).toBe('starting');
  act(() => void result.current.connect());
  expect(result.current.stage).toBe('checking');
  act(() => first('preparing_engine'));
  expect(result.current.stage).toBe('checking');
});
it.each(['not_installed', 'incompatible', 'unavailable'] as const)(
  'keeps %s closed and permits an explicit reconnect',
  async (state) => {
    const native = port();
    native.connect.mockResolvedValueOnce(state);
    const { result } = renderHook(() => useRuntimeConnection(native));
    await act(async () => Promise.resolve());
    expect(result.current).toMatchObject({ state, opened: false });
    await act(async () => result.current.connect());
    expect(result.current).toMatchObject({ state: 'ready', opened: true });
  },
);
it('recovers transient health failures by reading status without reconnecting or clearing the workspace', async () => {
  vi.useFakeTimers();
  const native = port();
  native.connect.mockRejectedValueOnce(new Error('offline'));
  const { result } = renderHook(() => useRuntimeConnection(native));
  await act(async () => Promise.resolve());
  expect(result.current).toMatchObject({ state: 'unavailable', opened: false });
  await act(async () => result.current.connect());
  native.status.mockRejectedValueOnce(new Error('lost IPC'));
  await act(async () => vi.advanceTimersByTimeAsync(2000));
  expect(result.current).toMatchObject({ state: 'unavailable', opened: true });
  await act(async () => vi.advanceTimersByTimeAsync(6000));
  expect(native.status).toHaveBeenCalledTimes(4);
  expect(result.current).toMatchObject({ state: 'ready', opened: true });
  expect(native.connect).toHaveBeenCalledTimes(2);
});
it('ignores a late connection from a previous port after replacement', async () => {
  const first = port(),
    second = port();
  const pending = Promise.withResolvers<RuntimeStatus>();
  first.connect.mockReturnValue(pending.promise);
  second.connect.mockResolvedValue('incompatible');
  const { result, rerender } = renderHook(({ native }) => useRuntimeConnection(native), {
    initialProps: { native: first },
  });
  rerender({ native: second });
  await act(async () => Promise.resolve());
  await act(async () => pending.resolve('ready'));
  expect(result.current).toMatchObject({ state: 'incompatible', opened: false });
});
it('ignores a late health reply from a replaced port and keeps only one current check', async () => {
  vi.useFakeTimers();
  const first = port(),
    second = port();
  const pending = Promise.withResolvers<RuntimeStatus>();
  first.status.mockReturnValue(pending.promise);
  const { result, rerender } = renderHook(({ native }) => useRuntimeConnection(native), {
    initialProps: { native: first },
  });
  await act(async () => Promise.resolve());
  await act(async () => vi.advanceTimersByTimeAsync(2000));
  rerender({ native: second });
  await act(async () => Promise.resolve());
  await act(async () => pending.resolve('unavailable'));
  expect(result.current.state).toBe('ready');
  await act(async () => vi.advanceTimersByTimeAsync(2000));
  expect(second.status).toHaveBeenCalledTimes(1);
  expect(first.status).toHaveBeenCalledTimes(1);
});

it('stops health polling when the opened Driver becomes incompatible', async () => {
  vi.useFakeTimers();
  const native = port();
  const { result, unmount } = renderHook(() => useRuntimeConnection(native));
  await act(async () => Promise.resolve());
  native.status.mockResolvedValue('incompatible');
  await act(async () => vi.advanceTimersByTimeAsync(2000));
  expect(result.current).toMatchObject({ state: 'incompatible', opened: true });
  await act(async () => vi.advanceTimersByTimeAsync(6000));
  expect(native.status).toHaveBeenCalledTimes(1);
  expect(native.connect).toHaveBeenCalledTimes(1);
  unmount();
  expect(vi.getTimerCount()).toBe(0);
});
