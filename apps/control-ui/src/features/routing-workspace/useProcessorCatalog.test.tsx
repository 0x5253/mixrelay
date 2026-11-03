import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { browserProcessorPaths } from '../../core/adapters/browser/processorPaths';
import type { ExternalProcessorCatalog } from '../../core/domain/control/processors';

import type { ProcessorCatalog } from './domain/processorCatalog';
import { useProcessorCatalog } from './useProcessorCatalog';

const empty: ExternalProcessorCatalog = { protocolVersion: 1, processors: [] };
const loaded: ExternalProcessorCatalog = {
  ...empty,
  processors: [
    {
      typeId: 'vst3:fixture',
      displayName: 'Fixture',
      vendor: 'MixRelay',
      format: 'vst3',
      version: '1',
      layouts: [
        {
          layoutId: 'layout-v1:stereo',
          inputs: [
            {
              id: 'audio',
              name: 'Main Input',
              kind: 'main',
              channels: 2,
              channelFormat: 'stereo',
            },
          ],
          outputs: [
            {
              id: 'audio',
              name: 'Main Output',
              kind: 'main',
              channels: 2,
              channelFormat: 'stereo',
            },
          ],
        },
      ],
      defaultLayoutId: 'layout-v1:stereo',
      parameters: [],
      hasEditor: true,
    },
  ],
};
function port() {
  return {
    getProcessors: vi.fn<() => Promise<ExternalProcessorCatalog>>().mockResolvedValue(loaded),
    rescanProcessors: vi
      .fn<(paths: string[]) => Promise<ExternalProcessorCatalog>>()
      .mockResolvedValue(loaded),
    selectProcessorDirectory: vi.fn<() => Promise<string | null>>().mockResolvedValue(null),
  };
}
async function settle() {
  await act(async () => {});
}
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  window.localStorage.clear();
});

describe('processor catalog native diagnostics', () => {
  it('retries native diagnostics with a real scan and clears them only after a clean result', async () => {
    const source = port();
    const diagnostic = { format: 'vst3', stage: 'scan', message: 'Timed out' } as const;
    source.getProcessors.mockResolvedValue({ ...loaded, diagnostics: [diagnostic] });
    source.rescanProcessors.mockRejectedValueOnce(new Error('offline'));
    const { result } = renderHook(() =>
      useProcessorCatalog(source, undefined, browserProcessorPaths),
    );
    await settle();
    expect(result.current.diagnostics).toEqual([diagnostic]);
    expect(result.current.processors).toEqual(loaded.processors);
    act(() => result.current.retry());
    await settle();
    expect(result.current.failure).toBe('catalog');
    expect(result.current.diagnostics).toEqual([diagnostic]);
    expect(result.current.processors).toEqual(loaded.processors);
    act(() => result.current.retry());
    await settle();
    expect(source.getProcessors).toHaveBeenCalledTimes(1);
    expect(source.rescanProcessors).toHaveBeenCalledTimes(2);
    expect(result.current.diagnostics).toEqual([]);
    expect(result.current.failure).toBeNull();
  });

  it('accepts a successful empty scan and ignores diagnostics from a replaced owner', async () => {
    const first = port(),
      second = port();
    const pending = Promise.withResolvers<ExternalProcessorCatalog>();
    first.rescanProcessors.mockReturnValue(pending.promise);
    second.getProcessors.mockResolvedValue(empty);
    const { result, rerender } = renderHook(
      ({ source }) => useProcessorCatalog(source, undefined, browserProcessorPaths),
      {
        initialProps: { source: first },
      },
    );
    await settle();
    act(() => result.current.refresh(false));
    await settle();
    rerender({ source: second });
    await settle();
    await act(async () =>
      pending.resolve({
        ...loaded,
        diagnostics: [{ format: 'au', stage: 'probe', pluginId: 'old', message: 'Denied' }],
      }),
    );
    expect(result.current.processors).toEqual([]);
    expect(result.current.diagnostics).toEqual([]);
    expect(result.current.failure).toBeNull();
  });
});

describe('processor catalog failure recovery', () => {
  it('shows an initial failure and retries without reporting an empty successful scan', async () => {
    const source = port();
    source.getProcessors.mockRejectedValueOnce(new Error('unavailable'));
    const { result } = renderHook(() =>
      useProcessorCatalog(source, undefined, browserProcessorPaths),
    );
    await settle();
    expect(result.current.failure).toBe('catalog');
    act(() => result.current.retry());
    await settle();
    expect(result.current.failure).toBeNull();
    expect(result.current.processors).toEqual(loaded.processors);
    expect(source.getProcessors).toHaveBeenCalledTimes(2);
  });

  it('preserves the last list and retries a failed chosen-directory scan without reopening the picker', async () => {
    const source = port();
    source.selectProcessorDirectory.mockResolvedValue('/fixture/vst3');
    source.rescanProcessors.mockRejectedValueOnce(new Error('scan failed'));
    const { result } = renderHook(() =>
      useProcessorCatalog(source, undefined, browserProcessorPaths),
    );
    await settle();
    act(() => result.current.refresh(true));
    await settle();
    expect(result.current.failure).toBe('catalog');
    expect(result.current.processors).toEqual(loaded.processors);
    act(() => result.current.retry());
    await settle();
    expect(source.selectProcessorDirectory).toHaveBeenCalledTimes(1);
    expect(source.rescanProcessors).toHaveBeenLastCalledWith(['/fixture/vst3']);
    expect(result.current.failure).toBeNull();
  });

  it('distinguishes picker failure, cancellation, and path-storage failure', async () => {
    const source = port();
    source.selectProcessorDirectory.mockRejectedValueOnce(new Error('denied'));
    const { result } = renderHook(() =>
      useProcessorCatalog(source, undefined, browserProcessorPaths),
    );
    await settle();
    act(() => result.current.refresh(true));
    await settle();
    expect(result.current.failure).toBe('directory');
    act(() => result.current.retry());
    await settle();
    expect(result.current.failure).toBe('directory'); // Cancelled retry does not claim recovery.
    expect(result.current.busy).toBe(false);
    expect(source.rescanProcessors).not.toHaveBeenCalled();
    source.selectProcessorDirectory.mockResolvedValue('/fixture');
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('quota');
    });
    act(() => result.current.refresh(true));
    await settle();
    expect(result.current.failure).toBe('paths');
    expect(source.rescanProcessors).not.toHaveBeenCalled();
  });
});

describe('processor catalog lifecycle and stored paths', () => {
  it('does not overlap a scan and ignores late old-port results or dialogs', async () => {
    const first = port(),
      second = port();
    const pending = Promise.withResolvers<ExternalProcessorCatalog>();
    first.rescanProcessors.mockReturnValue(pending.promise);
    const { result, rerender, unmount } = renderHook(
      ({ source }) => useProcessorCatalog(source, undefined, browserProcessorPaths),
      {
        initialProps: { source: first },
      },
    );
    await settle();
    act(() => {
      result.current.refresh(false);
      result.current.refresh(false);
    });
    await settle();
    expect(first.rescanProcessors).toHaveBeenCalledTimes(1);
    rerender({ source: second });
    await settle();
    await act(async () => pending.resolve(empty));
    expect(result.current.processors).toEqual(loaded.processors);
    const dialog = Promise.withResolvers<string | null>();
    second.selectProcessorDirectory.mockReturnValue(dialog.promise);
    act(() => result.current.refresh(true));
    await settle();
    unmount();
    await act(async () => dialog.resolve('/stale'));
    expect(window.localStorage.getItem('mixrelay.processorPaths.v1')).toBeNull();
    expect(second.rescanProcessors).not.toHaveBeenCalled();
  });

  it('reports damaged saved paths and recovers only after a valid explicit retry', async () => {
    window.localStorage.setItem('mixrelay.processorPaths.v1', '{broken');
    const source = port();
    const { result } = renderHook(() =>
      useProcessorCatalog(source, undefined, browserProcessorPaths),
    );
    await settle();
    expect(result.current.failure).toBe('paths');
    expect(source.getProcessors).not.toHaveBeenCalled();
    window.localStorage.setItem('mixrelay.processorPaths.v1', '[]');
    act(() => result.current.retry());
    await settle();
    expect(result.current.failure).toBeNull();
  });
});

it.each(['{}', '[42]'])('rejects structurally invalid persisted search paths %s', async (saved) => {
  window.localStorage.setItem('mixrelay.processorPaths.v1', saved);
  const source = port();
  const { result } = renderHook(() =>
    useProcessorCatalog(source, undefined, browserProcessorPaths),
  );
  await settle();
  expect(result.current.failure).toBe('paths');
  expect(source.getProcessors).not.toHaveBeenCalled();
});
it('reports an unavailable native directory picker without replacing the catalog', async () => {
  const source = port();
  const native = { getProcessors: source.getProcessors, rescanProcessors: source.rescanProcessors };
  const { result } = renderHook(() =>
    useProcessorCatalog(native, undefined, browserProcessorPaths),
  );
  await settle();
  act(() => result.current.refresh(true));
  await settle();
  expect(result.current.failure).toBe('directory');
  expect(result.current.processors).toEqual(loaded.processors);
});
it('does not publish a late rejected request or dispatch callbacks after unmount', async () => {
  const source = port();
  const pending = Promise.withResolvers<ExternalProcessorCatalog>();
  source.getProcessors.mockReturnValue(pending.promise);
  const { result, unmount } = renderHook(() =>
    useProcessorCatalog(source, undefined, browserProcessorPaths),
  );
  const refresh = result.current.refresh,
    retry = result.current.retry;
  await settle();
  expect(source.getProcessors).toHaveBeenCalledTimes(1);
  unmount();
  await act(async () => pending.reject(new Error('retired')));
  act(() => {
    refresh(false);
    retry();
  });
  expect(source.rescanProcessors).not.toHaveBeenCalled();
});

it('scans persisted nonempty search paths instead of reading a possibly stale default catalog', async () => {
  window.localStorage.setItem('mixrelay.processorPaths.v1', '["/User/Effects"]');
  const source = port();
  const { result } = renderHook(() =>
    useProcessorCatalog(source, undefined, browserProcessorPaths),
  );
  await settle();
  expect(source.rescanProcessors).toHaveBeenCalledExactlyOnceWith(['/User/Effects']);
  expect(source.getProcessors).not.toHaveBeenCalled();
  expect(result.current.processors).toEqual(loaded.processors);
});

it('uses the screen-owned catalog without starting a second native scan session', async () => {
  const source = port();
  const shared = {
    processors: loaded.processors,
    diagnostics: [],
    busy: false,
    failure: null,
    refresh: vi.fn(),
    retry: vi.fn(),
  };
  const { result, rerender } = renderHook(
    ({ catalog }) => useProcessorCatalog(source, catalog, browserProcessorPaths),
    {
      initialProps: { catalog: shared },
    },
  );
  await settle();
  act(() => {
    result.current.refresh(false);
    result.current.retry();
  });
  expect(shared.refresh).toHaveBeenCalledExactlyOnceWith(false);
  expect(shared.retry).toHaveBeenCalledTimes(1);
  expect(source.getProcessors).not.toHaveBeenCalled();
  const next = { ...shared, busy: true };
  rerender({ catalog: next });
  expect(result.current.busy).toBe(true);
  expect(source.rescanProcessors).not.toHaveBeenCalled();
});

it('invalidates its pending native request when a shared catalog takes ownership', async () => {
  const source = port();
  const pending = Promise.withResolvers<ExternalProcessorCatalog>();
  source.getProcessors.mockReturnValueOnce(pending.promise);
  const shared = {
    processors: [],
    diagnostics: [],
    busy: false,
    failure: null,
    refresh: vi.fn(),
    retry: vi.fn(),
  };
  const { result, rerender } = renderHook<
    ProcessorCatalog,
    { catalog: ProcessorCatalog | undefined }
  >(({ catalog }) => useProcessorCatalog(source, catalog, browserProcessorPaths), {
    initialProps: { catalog: undefined },
  });
  await settle();
  rerender({ catalog: shared });
  await act(async () => pending.resolve(loaded));
  expect(result.current.processors).toEqual([]);
  expect(result.current.busy).toBe(false);
  rerender({ catalog: undefined });
  await settle();
  expect(source.getProcessors).toHaveBeenCalledTimes(2);
  expect(result.current.processors).toEqual(loaded.processors);
});
