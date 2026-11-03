import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import { useCatalogTitle, useGraphCatalog } from './useGraphCatalog';

afterEach(() => vi.useRealTimers());

it('saves a trimmed graph title after quiet time and cancels the pending rename on unmount', async () => {
  vi.useFakeTimers();
  const port = new StorybookMockControlPort({
    graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      overruns: 0,
      underruns: 0,
    },
  });
  const rename = vi.spyOn(port, 'mutateGraphCatalog');
  const { result, unmount } = renderHook(() =>
    useCatalogTitle(useGraphCatalog(port), false, 'Untitled Graph'),
  );
  await act(async () => {});
  expect(result.current.draft).toBe('Untitled Graph');
  act(() => result.current.setDraft('  Voice  '));
  await act(async () => vi.advanceTimersByTime(499));
  expect(rename).not.toHaveBeenCalled();
  await act(async () => vi.advanceTimersByTime(1));
  expect(rename).toHaveBeenCalledWith(
    expect.objectContaining({ action: 'rename', id: 'graph-1', name: 'Voice' }),
  );
  expect((await port.getGraphCatalog()).graphs[0]?.name).toBe('Voice');
  act(() => result.current.setDraft('Another name'));
  unmount();
  await act(async () => vi.advanceTimersByTime(500));
  expect(rename).toHaveBeenCalledTimes(1);
});
