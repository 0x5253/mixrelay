import { act, renderHook } from '@testing-library/react';
import { type ReactNode, StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ProductUpdate } from '../../core/domain/updates';
import type { UpdatesPort } from '../../core/ports/UpdatesPort';

import { useUpdatesSession } from './useUpdatesSession';

const day = 24 * 60 * 60 * 1_000;
const release: ProductUpdate[] = [
  {
    product: 'mix-relay',
    installedVersion: '0.1.0',
    latestVersion: '0.1.1',
    state: 'available',
    notesUrl: null,
    downloads: [
      {
        os: 'macos',
        arch: 'universal',
        url: 'https://downloads.example/app.pkg',
        size: 12,
        sha256: 'a'.repeat(64),
      },
    ],
  },
];

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function port(check = vi.fn().mockResolvedValue(release)): UpdatesPort {
  return { check, openDownload: vi.fn().mockResolvedValue(undefined) };
}

afterEach(() => vi.useRealTimers());

describe('update check scheduling', () => {
  it('checks once at startup even with effect replay, then every 24 hours and cleans up', async () => {
    vi.useFakeTimers();
    const native = port();
    const { unmount } = renderHook(() => useUpdatesSession(native), {
      wrapper: ({ children }: { children: ReactNode }) => <StrictMode>{children}</StrictMode>,
    });
    await act(async () => Promise.resolve());
    expect(native.check).toHaveBeenCalledTimes(1);
    await act(async () => vi.advanceTimersByTimeAsync(day - 1));
    expect(native.check).toHaveBeenCalledTimes(1);
    await act(async () => vi.advanceTimersByTimeAsync(1));
    expect(native.check).toHaveBeenCalledTimes(2);
    unmount();
    await act(async () => vi.advanceTimersByTimeAsync(day));
    expect(native.check).toHaveBeenCalledTimes(2);
  });

  it('deduplicates manual and scheduled checks while one is pending', async () => {
    vi.useFakeTimers();
    const pending = deferred<ProductUpdate[]>();
    const native = port(vi.fn().mockReturnValue(pending.promise));
    const { result } = renderHook(() => useUpdatesSession(native));
    await act(async () => Promise.resolve());
    await act(async () => {
      await result.current.refresh();
      await result.current.refresh();
    });
    await act(async () => vi.advanceTimersByTimeAsync(day));
    expect(native.check).toHaveBeenCalledTimes(1);
    await act(async () => pending.resolve(release));
    expect(result.current.checking).toBe(false);
    await act(async () => result.current.refresh());
    expect(native.check).toHaveBeenCalledTimes(2);
  });

  it('does not schedule or send a check without a native port', async () => {
    vi.useFakeTimers();
    const { result } = renderHook(() => useUpdatesSession(null));
    await act(async () => {
      await result.current.refresh();
      await vi.advanceTimersByTimeAsync(day);
    });
    expect(result.current.checked).toBe(false);
    expect(result.current.products).toEqual([]);
    expect(vi.getTimerCount()).toBe(0);
  });
});

it('does not start a queued native check when its owner unmounts before the startup microtask', async () => {
  const native = port();
  const { unmount } = renderHook(() => useUpdatesSession(native));
  unmount();
  await act(async () => Promise.resolve());
  expect(native.check).not.toHaveBeenCalled();
});

describe('update result lifetime', () => {
  it('clears old installer URLs after a failed refresh and allows retry', async () => {
    const check = vi
      .fn()
      .mockResolvedValueOnce(release)
      .mockRejectedValueOnce('unavailable')
      .mockResolvedValueOnce([]);
    const native = port(check);
    const { result } = renderHook(() => useUpdatesSession(native));
    await act(async () => Promise.resolve());
    expect(result.current.products).toEqual(release);
    await act(async () => result.current.refresh());
    expect(result.current.products[0]?.downloads).toEqual([]);
    expect(result.current.products[0]?.installedVersion).toBe('0.1.0');
    expect(result.current.products[0]?.latestVersion).toBeNull();
    expect(result.current.failed).toBe(true);
    await act(async () => result.current.refresh());
    expect(result.current.failed).toBe(false);
    expect(result.current.checked).toBe(true);
  });

  it.each(['resolve', 'reject'])('ignores a late %s from the previous port', async (outcome) => {
    const pending = deferred<ProductUpdate[]>();
    const oldPort = port(vi.fn().mockReturnValue(pending.promise));
    const nextPort = port(vi.fn().mockResolvedValue([]));
    const { result, rerender } = renderHook(({ value }) => useUpdatesSession(value), {
      initialProps: { value: oldPort as UpdatesPort | null },
    });
    await act(async () => Promise.resolve());
    rerender({ value: nextPort });
    await act(async () => Promise.resolve());
    await act(async () => {
      if (outcome === 'resolve') pending.resolve(release);
      else pending.reject('private failure');
    });
    expect(result.current.products).toEqual([]);
    expect(result.current.failed).toBe(false);
    expect(result.current.checking).toBe(false);
    rerender({ value: null });
    expect(result.current.checked).toBe(false);
  });

  it('ignores a late check after unmount', async () => {
    const pending = deferred<ProductUpdate[]>();
    const native = port(vi.fn().mockReturnValue(pending.promise));
    const { result, unmount } = renderHook(() => useUpdatesSession(native));
    await act(async () => Promise.resolve());
    unmount();
    await act(async () => pending.resolve(release));
    expect(result.current.products).toEqual([]);
    expect(result.current.checked).toBe(false);
  });

  it('opens the selected installer once and isolates late opening failures from a new port', async () => {
    const pending = deferred<void>();
    const oldPort = port();
    vi.mocked(oldPort.openDownload).mockReturnValue(pending.promise);
    const nextPort = port();
    const { result, rerender } = renderHook(({ value }) => useUpdatesSession(value), {
      initialProps: { value: oldPort },
    });
    await act(async () => Promise.resolve());
    let operation!: Promise<void>;
    act(() => {
      operation = result.current.openDownload('mix-relay', 'https://downloads.example/app.pkg');
      void result.current.openDownload('mix-relay', 'https://downloads.example/app.pkg');
    });
    expect(oldPort.openDownload).toHaveBeenCalledTimes(1);
    rerender({ value: nextPort });
    await act(async () => {
      pending.reject('private failure');
      await operation;
    });
    expect(result.current.openFailed).toBe(false);
    expect(result.current.opening).toBeNull();
  });
});
