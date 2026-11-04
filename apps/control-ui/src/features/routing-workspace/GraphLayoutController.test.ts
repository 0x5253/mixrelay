import { afterEach, describe, expect, it, vi } from 'vitest';
import { monotonicClock, timerScheduler } from '../../core/adapters/browser/timers';
import type { GraphLayout, GraphLayoutResponse } from '../../core/domain/control/catalog';
import { catalogOf, graphWithNodes } from '../../test-support/portDefaults';
import { routingLayoutProjection } from './runtimeGraphProjection';

/** Every node the layouts in this file place. */
const layoutGraph = graphWithNodes(['a', 'b', 'concurrent', 'fresh', 'input', 'output']);

import {
  GraphLayoutController,
  type LayoutPort,
} from '../../core/application/workspace/GraphLayoutController';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
const layout = (x: number): GraphLayout => ({
  positions: { input: { x, y: 20 }, output: { x: 600, y: 80 } },
});
const saved = (x: number, layoutRevision = 1): GraphLayoutResponse => ({
  graphId: 'a',
  layoutRevision,
  layout: layout(x),
});
function fixture() {
  const getGraphLayout = vi.fn().mockResolvedValue(saved(10));
  const setGraphLayout = vi.fn().mockResolvedValue(saved(50, 2));
  const applyGraph = vi.fn();
  const port: LayoutPort = {
    getGraphLayout,
    setGraphLayout,
    getGraphCatalog: vi.fn(async () => catalogOf('a', layoutGraph)),
  };
  return {
    controller: new GraphLayoutController(
      port,
      'a',
      timerScheduler,
      monotonicClock,
      routingLayoutProjection,
    ),
    getGraphLayout,
    setGraphLayout,
    applyGraph,
  };
}
afterEach(() => vi.useRealTimers());

describe('presentation checkpoint controller', () => {
  it('flushes the last resize on graph switch after the current write completes', async () => {
    vi.useFakeTimers();
    const f = fixture();
    await f.controller.reload();
    const pending = deferred<GraphLayoutResponse>();
    f.setGraphLayout.mockReturnValueOnce(pending.promise);
    f.controller.update(layout(20));
    await vi.advanceTimersByTimeAsync(400);
    const resized = { ...layout(30), sizes: { input: { width: 400, height: 220 } } };
    f.controller.update(resized);
    f.controller.dispose();
    pending.resolve(saved(20, 2));
    await vi.advanceTimersByTimeAsync(0);
    expect(f.setGraphLayout).toHaveBeenLastCalledWith('a', {
      expectedLayoutRevision: 2,
      layout: resized,
    });
    expect(f.setGraphLayout).toHaveBeenCalledTimes(2);
    expect(f.applyGraph).not.toHaveBeenCalled();
  });

  it('restores separate input/output positions and debounces 500 edits into one audio-free save', async () => {
    vi.useFakeTimers();
    const f = fixture();
    await f.controller.reload();
    expect(f.controller.snapshot().layout).toEqual(layout(10));
    for (let x = 1; x <= 500; x++) {
      f.controller.update(layout(x));
      await vi.advanceTimersByTimeAsync(1);
    }
    expect(f.setGraphLayout).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(400);
    expect(f.setGraphLayout).toHaveBeenCalledExactlyOnceWith('a', {
      expectedLayoutRevision: 1,
      layout: layout(500),
    });
    expect(f.controller.snapshot().dirty).toBe(false);
    expect(f.applyGraph).not.toHaveBeenCalled();
    f.controller.dispose();
  });

  it('serializes in-flight writes and keeps the most recent positions and viewport', async () => {
    vi.useFakeTimers();
    const f = fixture();
    await f.controller.reload();
    const first = deferred<GraphLayoutResponse>();
    f.setGraphLayout.mockReturnValueOnce(first.promise);
    f.controller.update(layout(20));
    await vi.advanceTimersByTimeAsync(400);
    const last = { ...layout(80), viewport: { x: -100, y: 200, zoom: 0.75 } };
    f.controller.update(last);
    await vi.advanceTimersByTimeAsync(400);
    expect(f.setGraphLayout).toHaveBeenCalledTimes(1);
    first.resolve(saved(20, 8));
    await vi.advanceTimersByTimeAsync(0);
    expect(f.setGraphLayout).toHaveBeenLastCalledWith('a', {
      expectedLayoutRevision: 8,
      layout: last,
    });
    expect(f.controller.snapshot().layout).toEqual(last);
    f.controller.dispose();
  });
});

describe('presentation checkpoint recovery', () => {
  it('preserves failed/conflicting drafts until explicitly saved against a fresh baseline', async () => {
    vi.useFakeTimers();
    const f = fixture();
    await f.controller.reload();
    f.setGraphLayout.mockRejectedValueOnce(new Error('conflict'));
    f.controller.update(layout(80));
    await vi.advanceTimersByTimeAsync(400);
    expect(f.controller.snapshot()).toMatchObject({ error: true, dirty: true, layout: layout(80) });
    f.controller.update(layout(90));
    await vi.advanceTimersByTimeAsync(2000);
    expect(f.setGraphLayout).toHaveBeenCalledTimes(1);
    f.getGraphLayout.mockResolvedValueOnce(saved(400, 12));
    await f.controller.retry();
    await vi.advanceTimersByTimeAsync(0);
    expect(f.setGraphLayout).toHaveBeenLastCalledWith('a', {
      expectedLayoutRevision: 12,
      layout: layout(90),
    });
    expect(f.controller.snapshot()).toMatchObject({ dirty: false, error: false });
    f.controller.dispose();
  });

  it('allows explicit saved-layout reload without modifying audio and ignores wrong graph IDs', async () => {
    const f = fixture();
    await f.controller.reload();
    f.controller.update(layout(50));
    f.getGraphLayout.mockResolvedValueOnce(saved(700, 5));
    await f.controller.reload();
    expect(f.controller.snapshot()).toMatchObject({
      dirty: false,
      layout: layout(700),
      restoreVersion: 2,
    });
    f.getGraphLayout.mockResolvedValueOnce({ ...saved(900), graphId: 'another' });
    await f.controller.reload();
    expect(f.controller.snapshot()).toMatchObject({ error: true, layout: layout(700) });
    expect(f.applyGraph).not.toHaveBeenCalled();
    f.controller.dispose();
  });

  it('bounds a hung load and fences late replies after disposal or a newer reload', async () => {
    vi.useFakeTimers();
    const f = fixture();
    const old = deferred<GraphLayoutResponse>();
    f.getGraphLayout.mockReturnValueOnce(old.promise);
    const loading = f.controller.reload();
    await vi.advanceTimersByTimeAsync(10_000);
    await loading;
    expect(f.controller.snapshot()).toMatchObject({ error: true, loaded: false, loading: false });
    await f.controller.reload();
    old.resolve(saved(999));
    await vi.advanceTimersByTimeAsync(0);
    expect(f.controller.snapshot().layout).toEqual(layout(10));
    f.controller.update(layout(80));
    f.controller.dispose();
    f.controller.update(layout(90));
    await vi.advanceTimersByTimeAsync(400);
    expect(f.setGraphLayout).toHaveBeenCalledExactlyOnceWith('a', {
      expectedLayoutRevision: 1,
      layout: layout(80),
    });
  });
});
