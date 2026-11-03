import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { GraphApplicationQueue } from '../../core/application/workspace/GraphApplicationQueue';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { GraphNodeControls } from '../../core/domain/control/graph';
import { controlState, statePush } from '../../test-support/controlTelemetry';
import { useNodeControls } from './useNodeControls';

afterEach(() => vi.useRealTimers());
const snapshot: GraphNodeControls = { graphId: 'a', revision: 2, nodes: [] };
function queue() {
  return {
    canPublishParameters: true,
    refresh: vi.fn(),
  } as unknown as GraphApplicationQueue<AudioGraphModel>;
}

it('replays committed push after local edits finish, fences reads and removes missing graph controls', async () => {
  vi.useFakeTimers();
  const source = statePush();
  const pending = Promise.withResolvers<GraphNodeControls>();
  const getNodeControls = vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValue(snapshot);
  const port = { ...source, getNodeControls };
  const application = queue();
  const view = renderHook(({ idle }) => useNodeControls(port, 'a', 2, idle, application), {
    initialProps: { idle: true },
  });
  view.rerender({ idle: false });
  act(() => source.push(controlState()));
  expect(view.result.current).toBeNull();
  await act(async () => pending.resolve({ ...snapshot, revision: 99 }));
  view.rerender({ idle: true });
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(view.result.current).toEqual(snapshot);
  expect(getNodeControls).toHaveBeenCalledOnce();
  const missing = controlState(2);
  missing.controls = null;
  act(() => source.push(missing));
  expect(view.result.current).toBeNull();
  act(() => source.push());
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(getNodeControls).toHaveBeenCalledTimes(2);
  view.unmount();
});

it('reloads a replacement engine even when its revision repeats the previous engine revision', async () => {
  vi.useFakeTimers();
  const source = statePush();
  const port = {
    ...source,
    getNodeControls: vi.fn().mockResolvedValue(snapshot),
  };
  const application = queue();
  const view = renderHook(() => useNodeControls(port, 'a', 2, true, application));
  act(() => source.push(controlState(), 'engine-a'));
  expect(application.refresh).not.toHaveBeenCalled();
  act(() => source.push());
  act(() => source.push(controlState(), 'engine-b'));
  expect(application.refresh).toHaveBeenCalledOnce();
  act(() => source.push(controlState(2), 'engine-b'));
  expect(application.refresh).toHaveBeenCalledOnce();
  view.unmount();
});
it('reloads external changes through the graph owner and pauses while a local edit is pending', async () => {
  vi.useFakeTimers();
  const getNodeControls = vi.fn().mockResolvedValue(snapshot);
  const port = { getNodeControls };
  const application = queue();
  const view = renderHook(({ idle }) => useNodeControls(port, 'a', 1, idle, application), {
    initialProps: { idle: false },
  });
  expect(getNodeControls).not.toHaveBeenCalled();
  view.rerender({ idle: true });
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(application.refresh).toHaveBeenCalledTimes(1);
  expect(view.result.current).toEqual(snapshot);
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});
it('discards a late old-graph response and does not issue overlapping polls', async () => {
  vi.useFakeTimers();
  const pending = Promise.withResolvers<GraphNodeControls>();
  const getNodeControls = vi
    .fn()
    .mockReturnValueOnce(pending.promise)
    .mockResolvedValue({ ...snapshot, graphId: 'b' });
  const port = { getNodeControls };
  const application = queue();
  const view = renderHook(({ graphId }) => useNodeControls(port, graphId, 2, true, application), {
    initialProps: { graphId: 'a' },
  });
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(getNodeControls).toHaveBeenCalledTimes(1);
  view.rerender({ graphId: 'b' });
  await act(async () => pending.resolve(snapshot));
  expect(view.result.current?.graphId).toBe('b');
  expect(application.refresh).not.toHaveBeenCalled();
  view.unmount();
});

it('clears stale actionable controls on failure and ignores a rejection after disposal', async () => {
  vi.useFakeTimers();
  const pending = Promise.withResolvers<GraphNodeControls>();
  const getNodeControls = vi
    .fn()
    .mockResolvedValueOnce(snapshot)
    .mockRejectedValueOnce(new Error('offline'))
    .mockReturnValueOnce(pending.promise);
  const application = queue();
  const port = { getNodeControls };
  const view = renderHook(() => useNodeControls(port, 'a', 2, true, application));
  await act(async () => {});
  expect(view.result.current).toEqual(snapshot);
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(view.result.current).toBeNull();
  await act(() => vi.advanceTimersByTimeAsync(1000));
  view.unmount();
  await act(async () => pending.reject(new Error('disposed')));
  expect(vi.getTimerCount()).toBe(0);
  expect(application.refresh).not.toHaveBeenCalled();
});
