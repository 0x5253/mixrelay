import { act, renderHook, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { MixRelayStatus } from '../../core/domain/control/status';
import { controlState, statePush } from '../../test-support/controlTelemetry';
import { useAudioStatus } from './useAudioStatus';

const status: MixRelayStatus = {
  protocolVersion: 1,
  driverState: 'ready',
  sampleRate: 48000,
  bufferFrames: 128,
  underruns: 0,
  overruns: 0,
  enginePower: { enabled: true, transitioning: false },
};

it('uses pushed power, fences late fallback responses and clears lost health before polling resumes', async () => {
  vi.useFakeTimers();
  try {
    const source = statePush();
    const pending = Promise.withResolvers<MixRelayStatus>();
    const getStatus = vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValue(status);
    const port = { ...source, getStatus };
    const hook = renderHook(() => useAudioStatus(port));
    act(() => source.push(controlState()));
    await act(async () => pending.resolve({ ...status, driverState: 'unavailable' }));
    expect(hook.result.current?.driverState).toBe('ready');
    await act(() => vi.advanceTimersByTimeAsync(1000));
    expect(getStatus).toHaveBeenCalledOnce();
    act(() => source.push());
    expect(hook.result.current).toBeNull();
    await act(() => vi.advanceTimersByTimeAsync(1000));
    expect(hook.result.current).toEqual(status);
    hook.unmount();
    source.push(controlState());
  } finally {
    vi.useRealTimers();
  }
});

it('retains the status while refreshing, then adopts the acknowledged pause', async () => {
  let resolve!: (value: MixRelayStatus) => void;
  const pending = new Promise<MixRelayStatus>((done) => {
    resolve = done;
  });
  const port = {
    getStatus: vi.fn().mockResolvedValueOnce(status).mockReturnValueOnce(pending),
  };
  const hook = renderHook(({ refresh }) => useAudioStatus(port, refresh), {
    initialProps: { refresh: 0 },
  });
  await waitFor(() => expect(hook.result.current).toEqual(status));
  hook.rerender({ refresh: 1 });
  expect(hook.result.current).toEqual(status);
  const paused = { ...status, enginePower: { enabled: false, transitioning: false } };
  await act(async () => resolve(paused));
  // Unchanged parts keep their identity; the reply's content is adopted.
  expect(hook.result.current).toEqual(paused);
});

it('never retains ready health for a replacement or failed service', async () => {
  const first = { getStatus: vi.fn().mockResolvedValue(status) };
  const next = {
    getStatus: vi.fn().mockRejectedValue(new Error('offline')),
  };
  const hook = renderHook(({ port, refresh }) => useAudioStatus(port, refresh), {
    initialProps: { port: first, refresh: 0 },
  });
  await waitFor(() => expect(hook.result.current).toEqual(status));
  vi.mocked(first.getStatus).mockRejectedValueOnce(new Error('offline'));
  hook.rerender({ port: first, refresh: 1 });
  await waitFor(() => expect(hook.result.current).toBeNull());
  hook.rerender({ port: next, refresh: 2 });
  expect(hook.result.current).toBeNull();
  await waitFor(() => expect(next.getStatus).toHaveBeenCalledOnce());
});

it('polls after completion and ignores late replies from a cancelled refresh', async () => {
  vi.useFakeTimers();
  let resolve!: (value: MixRelayStatus) => void;
  const late = new Promise<MixRelayStatus>((done) => {
    resolve = done;
  });
  const port = {
    getStatus: vi
      .fn()
      .mockResolvedValueOnce(status)
      .mockReturnValueOnce(late)
      .mockResolvedValue(status),
  };
  const hook = renderHook(({ refresh }) => useAudioStatus(port, refresh), {
    initialProps: { refresh: 0 },
  });
  try {
    await act(async () => {});
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(port.getStatus).toHaveBeenCalledTimes(2);
    expect(hook.result.current).toEqual(status);
    hook.rerender({ refresh: 1 });
    await act(async () => {});
    await act(async () => resolve({ ...status, driverState: 'unavailable' }));
    expect(hook.result.current).toEqual(status);
    hook.unmount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(port.getStatus).toHaveBeenCalledTimes(3);
  } finally {
    hook.unmount();
    vi.useRealTimers();
  }
});

it('ignores a failed status read from a replaced service instead of hiding the current service health', async () => {
  let reject: ((error: Error) => void) | undefined;
  const old = {
    getStatus: vi.fn().mockReturnValue(
      new Promise((_resolve, fail) => {
        reject = fail;
      }),
    ),
  };
  const current = { getStatus: vi.fn().mockResolvedValue(status) };
  const hook = renderHook(({ port }) => useAudioStatus(port), { initialProps: { port: old } });
  hook.rerender({ port: current });
  await waitFor(() => expect(hook.result.current).toEqual(status));
  await act(async () => {
    if (!reject) throw new Error('Previous status request was not started');
    reject(new Error('Old service disconnected'));
  });
  expect(hook.result.current).toEqual(status);
});

it('keeps an unchanged poll reply, but marks the first reply after a refresh as new', async () => {
  vi.useFakeTimers();
  try {
    // The same object every time, as fixture ports do.
    const port = { getStatus: vi.fn(async () => status) };
    const hook = renderHook(({ refresh }) => useAudioStatus(port, refresh), {
      initialProps: { refresh: 0 },
    });
    await act(async () => vi.advanceTimersByTimeAsync(0));
    const first = hook.result.current;
    expect(first).toEqual(status);
    // An equal follow-up reply renders nothing new.
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    expect(hook.result.current).toBe(first);
    // An acknowledgement refresh is a new observation even when its content is equal.
    hook.rerender({ refresh: 1 });
    await act(async () => vi.advanceTimersByTimeAsync(0));
    expect(hook.result.current).toEqual(status);
    expect(hook.result.current).not.toBe(first);
  } finally {
    vi.useRealTimers();
  }
});
