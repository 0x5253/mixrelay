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

const layout = (x: number): GraphLayout => ({ positions: { input: { x, y: 10 } } });
const saved = (x = 10): GraphLayoutResponse => ({
  graphId: 'a',
  layoutRevision: 1,
  layout: layout(x),
});
function fixture() {
  const read = vi.fn().mockResolvedValue(saved());
  const write = vi.fn().mockResolvedValue(saved(20));
  const port: LayoutPort = {
    getGraphLayout: read,
    setGraphLayout: write,
    getGraphCatalog: vi.fn(async () => catalogOf('a', layoutGraph)),
  };
  return {
    port,
    read,
    write,
    controller: new GraphLayoutController(
      port,
      'a',
      timerScheduler,
      monotonicClock,
      routingLayoutProjection,
    ),
  };
}
afterEach(() => vi.useRealTimers());

it.each([undefined, ''])('leaves a controller without a graph (%j) inert', async (graphId) => {
  const f = fixture();
  const controller = new GraphLayoutController(
    f.port,
    graphId,
    timerScheduler,
    monotonicClock,
    routingLayoutProjection,
  );
  expect(controller.enabled).toBe(false);
  await controller.reload();
  controller.update(layout(20));
  await controller.retry();
  controller.dispose();
  expect(f.read).not.toHaveBeenCalled();
  expect(f.write).not.toHaveBeenCalled();
  expect(controller.snapshot()).toMatchObject({ loaded: true, dirty: false, error: false });
});

it('notifies subscribers and removes unsubscribed listeners without saving unchanged layouts', async () => {
  vi.useFakeTimers();
  const f = fixture();
  const listener = vi.fn();
  const unsubscribe = f.controller.subscribe(listener);
  await f.controller.reload();
  expect(listener).toHaveBeenCalledTimes(3);
  unsubscribe();
  f.controller.update(layout(10));
  await vi.advanceTimersByTimeAsync(400);
  expect(f.write).not.toHaveBeenCalled();
  await f.controller.retry();
  expect(f.read).toHaveBeenCalledTimes(2);
  expect(listener).toHaveBeenCalledTimes(3);
  f.controller.dispose();
});

describe('layout load ownership', () => {
  it.each(['accept', 'reject', 'dispose'] as const)(
    'fences an obsolete loading response: %s',
    async (outcome) => {
      const f = fixture();
      const old = Promise.withResolvers<GraphLayoutResponse>();
      f.read.mockReturnValueOnce(old.promise);
      f.controller.update(layout(90));
      expect(f.controller.snapshot().dirty).toBe(false);
      const request = f.controller.reload();
      f.controller.update(layout(90));
      await f.controller.retry();
      expect(f.read).toHaveBeenCalledTimes(1);
      if (outcome === 'dispose') f.controller.dispose();
      else await f.controller.reload();
      const before = f.controller.snapshot();
      if (outcome === 'reject') old.reject(new Error('old read'));
      else old.resolve(saved(99));
      await request;
      expect(f.controller.snapshot()).toBe(before);
      expect(f.write).not.toHaveBeenCalled();
      f.controller.dispose();
    },
  );

  it('keeps an initial failure retryable and avoids writing that failed draft during disposal', async () => {
    const f = fixture();
    f.read.mockRejectedValueOnce(new Error('offline'));
    await f.controller.reload();
    expect(f.controller.snapshot()).toMatchObject({ error: true, loading: false, loaded: false });
    f.controller.dispose();
    await f.controller.retry();
    expect(f.read).toHaveBeenCalledTimes(1);
    expect(f.write).not.toHaveBeenCalled();
    await f.controller.reload();
    expect(f.controller.snapshot()).toMatchObject({ error: false, loaded: true });
    f.controller.dispose();
  });
});

describe('layout write lifetime', () => {
  it.each(['accept', 'reject'] as const)(
    'preserves a newer reload when an old save %s arrives',
    async (outcome) => {
      vi.useFakeTimers();
      const f = fixture();
      await f.controller.reload();
      const old = Promise.withResolvers<GraphLayoutResponse>();
      f.write.mockReturnValueOnce(old.promise);
      f.controller.update(layout(20));
      await vi.advanceTimersByTimeAsync(400);
      await f.controller.retry();
      expect(f.read).toHaveBeenCalledTimes(1);
      await f.controller.reload();
      const before = f.controller.snapshot();
      if (outcome === 'accept') old.resolve(saved(20));
      else old.reject(new Error('old write'));
      await vi.advanceTimersByTimeAsync(0);
      expect(f.controller.snapshot()).toBe(before);
      f.controller.dispose();
    },
  );

  it('rejects a response for another graph without discarding the local draft', async () => {
    vi.useFakeTimers();
    const f = fixture();
    await f.controller.reload();
    f.write.mockResolvedValueOnce({ ...saved(20), graphId: 'wrong' });
    f.controller.update(layout(20));
    await vi.advanceTimersByTimeAsync(400);
    expect(f.controller.snapshot()).toMatchObject({ error: true, dirty: true, layout: layout(20) });
    f.controller.dispose();
    expect(f.write).toHaveBeenCalledTimes(1);
  });

  it('bounds a stalled writer and ignores its later response', async () => {
    vi.useFakeTimers();
    const f = fixture();
    await f.controller.reload();
    const old = Promise.withResolvers<GraphLayoutResponse>();
    f.write.mockReturnValueOnce(old.promise);
    f.controller.update(layout(20));
    await vi.advanceTimersByTimeAsync(10_400);
    expect(f.controller.snapshot()).toMatchObject({ error: true, saving: false, dirty: true });
    const before = f.controller.snapshot();
    old.resolve(saved(20));
    await vi.advanceTimersByTimeAsync(0);
    expect(f.controller.snapshot()).toBe(before);
    f.controller.dispose();
  });
});

describe('explicit layout rebasing', () => {
  it.each(['accept', 'reject'] as const)(
    'fences a retry %s after a newer reload',
    async (outcome) => {
      vi.useFakeTimers();
      const f = fixture();
      await f.controller.reload();
      f.controller.update(layout(20));
      const old = Promise.withResolvers<GraphLayoutResponse>();
      f.read.mockReturnValueOnce(old.promise);
      const retry = f.controller.retry();
      // Let the retry read the catalog and start its layout read before the newer reload.
      await vi.advanceTimersByTimeAsync(0);
      await f.controller.reload();
      const before = f.controller.snapshot();
      if (outcome === 'accept') old.resolve(saved(99));
      else old.reject(new Error('old retry'));
      await retry;
      expect(f.controller.snapshot()).toBe(before);
      expect(f.write).not.toHaveBeenCalled();
      f.controller.dispose();
    },
  );

  it('keeps a failed rebase unsaved, then explicitly saves after a healthy rebase', async () => {
    vi.useFakeTimers();
    const f = fixture();
    await f.controller.reload();
    f.controller.update(layout(20));
    f.read.mockRejectedValueOnce(new Error('cannot rebase'));
    await f.controller.retry();
    expect(f.controller.snapshot()).toMatchObject({ loading: false, error: true, dirty: true });
    expect(f.write).not.toHaveBeenCalled();
    await f.controller.retry();
    await vi.advanceTimersByTimeAsync(0);
    expect(f.write).toHaveBeenCalledTimes(1);
    f.controller.dispose();
  });
});
