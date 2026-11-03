import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { monotonicClock, timerScheduler } from '../../core/adapters/browser/timers';
import type {
  GraphCatalog,
  GraphLayout,
  GraphLayoutResponse,
} from '../../core/domain/control/catalog';
import { ControlPortError } from '../../core/domain/control/errors';
import { catalogOf, graphWithNodes } from '../../test-support/portDefaults';
import { routingLayoutProjection } from './model';

/** Every node the layouts in this file place. */
const layoutGraph = graphWithNodes(['a', 'b', 'concurrent', 'fresh', 'input', 'output']);

import {
  GraphLayoutController,
  type LayoutPort,
} from '../../core/application/workspace/GraphLayoutController';
import { rebaseGraphLayout } from '../../core/domain/graph/rebaseGraphLayout';

const base: GraphLayout = {
  positions: { a: { x: 10, y: 20 }, deleted: { x: 30, y: 40 } },
  sizes: { a: { width: 228, height: 120 } },
  viewport: { x: 1, y: 2, zoom: 1 },
};
const local: GraphLayout = {
  positions: { a: { x: 80, y: 20 }, deleted: { x: 90, y: 40 }, fresh: { x: 5, y: 6 } },
  sizes: { a: { width: 320, height: 120 }, deleted: { width: 228, height: 120 } },
  viewport: { x: 1, y: 2, zoom: 0.5 },
};
const remote: GraphLayout = {
  positions: { a: { x: 60, y: 100 }, concurrent: { x: 200, y: 300 } },
  sizes: { a: { width: 420, height: 240 } },
  viewport: { x: -80, y: 30, zoom: 0.75 },
};
const merged: GraphLayout = {
  positions: { a: { x: 80, y: 100 }, concurrent: { x: 200, y: 300 }, fresh: { x: 5, y: 6 } },
  sizes: { a: { width: 320, height: 240 } },
  viewport: { x: -80, y: 30, zoom: 0.5 },
};
const response = (layout: GraphLayout, layoutRevision = 1): GraphLayoutResponse => ({
  graphId: 'g',
  layoutRevision,
  layout,
});
const stale = (parameters = { resource: 'graph_layout', graphId: 'g' }) =>
  new ControlPortError(409, { code: 'graph_revision_stale', parameters });
function fixture() {
  const read = vi.fn().mockResolvedValue(response(base));
  const write = vi
    .fn()
    .mockImplementation(async (_id, request) =>
      response(request.layout, request.expectedLayoutRevision + 1),
    );
  const port: LayoutPort = {
    getGraphLayout: read,
    setGraphLayout: write,
    getGraphCatalog: vi.fn(async () => catalogOf('g', layoutGraph)),
  };
  return {
    port,
    read,
    write,
    controller: new GraphLayoutController(
      port,
      'g',
      timerScheduler,
      monotonicClock,
      routingLayoutProjection,
    ),
  };
}
beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());
const flush = () => vi.advanceTimersByTimeAsync(400);

it('merges changed fields, preserves concurrent additions and never revives remotely removed geometry', () => {
  expect(rebaseGraphLayout(base, local, remote)).toEqual(merged);
  expect(remote.positions.a).toEqual({ x: 60, y: 100 });
  const collision = { positions: { fresh: { x: 9, y: 9 } }, viewport: { x: 3, y: 4, zoom: 2 } };
  expect(
    rebaseGraphLayout({ positions: {} }, { positions: { fresh: { x: 1, y: 2 } } }, collision),
  ).toEqual(collision);
  expect(rebaseGraphLayout(base, { positions: {}, sizes: {}, viewport: undefined }, base)).toEqual({
    positions: {},
  });
  expect(rebaseGraphLayout(base, local, { positions: {} })).toEqual({
    positions: { fresh: { x: 5, y: 6 } },
    viewport: local.viewport,
  });
  expect(rebaseGraphLayout(base, base, { positions: {} })).toEqual({ positions: {} });
});

it('automatically retries only one definite layout rejection using the three-way merge', async () => {
  const f = fixture();
  await f.controller.reload();
  f.write.mockRejectedValueOnce(stale());
  f.read.mockResolvedValueOnce(response(remote, 8));
  f.controller.update(local);
  await flush();
  expect(f.write).toHaveBeenCalledTimes(2);
  expect(f.write).toHaveBeenLastCalledWith('g', { expectedLayoutRevision: 8, layout: merged });
  expect(f.controller.snapshot()).toMatchObject({
    layout: merged,
    error: false,
    dirty: false,
    restoreVersion: 2,
  });
  f.controller.dispose();
});

it.each([
  new Error('unknown ack'),
  stale({ resource: 'graph', graphId: 'g' }),
  stale({ resource: 'graph_layout', graphId: 'other' }),
])('does not automatically retry an ambiguous or unrelated failure %s', async (error) => {
  const f = fixture();
  await f.controller.reload();
  f.write.mockRejectedValueOnce(error);
  f.controller.update(local);
  await flush();
  expect(f.read).toHaveBeenCalledTimes(1);
  expect(f.write).toHaveBeenCalledTimes(1);
  expect(f.controller.snapshot()).toMatchObject({ error: true, dirty: true, layout: local });
  f.controller.dispose();
});

it('merges two ordinary layout revision advances without requiring a reload decision', async () => {
  const f = fixture();
  await f.controller.reload();
  f.write.mockRejectedValueOnce(stale()).mockRejectedValueOnce(stale());
  f.read.mockResolvedValue(response(remote, 8));
  f.controller.update(local);
  await vi.advanceTimersByTimeAsync(600);
  expect(f.write).toHaveBeenCalledTimes(3);
  expect(f.write).toHaveBeenLastCalledWith('g', { expectedLayoutRevision: 8, layout: merged });
  expect(f.controller.snapshot()).toMatchObject({ error: false, dirty: false, layout: merged });
  f.controller.dispose();
});

it('preserves its merged draft when continuous layout contention exceeds the recovery deadline', async () => {
  const f = fixture();
  await f.controller.reload();
  f.write.mockRejectedValue(stale());
  f.read.mockResolvedValue(response(remote, 8));
  f.controller.update(local);
  await vi.advanceTimersByTimeAsync(10_400);
  expect(f.write.mock.calls.length).toBeGreaterThan(2);
  expect(f.controller.snapshot()).toMatchObject({ error: true, dirty: true, layout: merged });
  f.controller.dispose();
  expect(vi.getTimerCount()).toBe(0);
});

it.each(['reload', 'dispose'] as const)(
  'cancels layout contention backoff after %s without publishing a stale write',
  async (operation) => {
    const f = fixture();
    await f.controller.reload();
    f.write.mockRejectedValue(stale());
    f.read.mockResolvedValue(response(remote, 8));
    f.controller.update(local);
    await flush();
    expect(f.write).toHaveBeenCalledTimes(2);
    if (operation === 'reload') await f.controller.reload();
    else f.controller.dispose();
    const snapshot = f.controller.snapshot();
    await vi.advanceTimersByTimeAsync(1000);
    expect(f.write).toHaveBeenCalledTimes(2);
    if (operation === 'reload') expect(f.controller.snapshot()).toBe(snapshot);
    else
      expect(f.controller.snapshot()).toMatchObject({
        error: true,
        dirty: true,
        saving: false,
        layout: merged,
      });
    expect(vi.getTimerCount()).toBe(0);
    f.controller.dispose();
  },
);

it('does not replay an unknown acknowledgement received after an automatic layout rebase', async () => {
  const f = fixture();
  await f.controller.reload();
  f.write.mockRejectedValueOnce(stale()).mockRejectedValueOnce(new Error('lost rebased reply'));
  f.read.mockResolvedValueOnce(response(remote, 8));
  f.controller.update(local);
  await vi.advanceTimersByTimeAsync(2000);
  expect(f.write).toHaveBeenCalledTimes(2);
  expect(f.controller.snapshot()).toMatchObject({ error: true, dirty: true, layout: merged });
  f.controller.dispose();
});

it('merges the latest edit received while the rejected write is in flight', async () => {
  const f = fixture();
  await f.controller.reload();
  const pending = Promise.withResolvers<GraphLayoutResponse>();
  f.write.mockReturnValueOnce(pending.promise);
  f.controller.update(local);
  await flush();
  f.controller.update({ ...local, viewport: { x: 1, y: 2, zoom: 0.3 } });
  f.read.mockResolvedValueOnce(response(remote, 8));
  pending.reject(stale());
  await vi.advanceTimersByTimeAsync(0);
  expect(f.write).toHaveBeenLastCalledWith('g', {
    expectedLayoutRevision: 8,
    layout: { ...merged, viewport: { x: -80, y: 30, zoom: 0.3 } },
  });
  f.controller.dispose();
});

it('filters an externally deleted new node through authoritative UI identities', async () => {
  const f = fixture();
  await f.controller.reload();
  const catalog = {
    activeId: 'g',
    layout: remote,
    layoutRevision: 8,
    document: {
      schemaVersion: 2,
      revision: 3,
      edges: [],
      nodes: [
        { id: 'a', type: 'gain', config: { channels: 2, gainDb: 0 } },
        { id: 'concurrent', type: 'gain', config: { channels: 2, gainDb: 0 } },
      ],
    },
  };
  f.port.getGraphCatalog = vi.fn().mockResolvedValue(catalog);
  f.write.mockRejectedValueOnce(stale());
  f.controller.update(local);
  await flush();
  const { fresh: _fresh, ...positions } = merged.positions;
  expect(f.write).toHaveBeenLastCalledWith('g', {
    expectedLayoutRevision: 8,
    layout: { ...merged, positions },
  });
  expect(f.read).toHaveBeenCalledTimes(1);
  f.controller.dispose();
});

it('refuses a catalogue for another active graph and preserves the failed draft', async () => {
  const f = fixture();
  await f.controller.reload();
  f.port.getGraphCatalog = vi.fn().mockResolvedValue({ activeId: 'other' });
  f.write.mockRejectedValueOnce(stale());
  f.controller.update(local);
  await flush();
  expect(f.write).toHaveBeenCalledTimes(1);
  expect(f.controller.snapshot()).toMatchObject({ error: true, layout: local });
  f.controller.dispose();
});

it.each([
  ['reload', 'layout'],
  ['dispose', 'layout'],
  ['reload', 'catalog'],
  ['dispose', 'catalog'],
] as const)('fences a stale recovery %s during its %s read', async (operation, boundary) => {
  const f = fixture();
  await f.controller.reload();
  const pending = Promise.withResolvers<void>();
  if (boundary === 'catalog')
    f.port.getGraphCatalog = vi.fn(async (): Promise<GraphCatalog> => {
      await pending.promise;
      return {
        protocolVersion: 1,
        catalogRevision: 8,
        activeId: 'g',
        graphs: [{ id: 'g', name: 'Graph' }],
        document: { schemaVersion: 2, revision: 3, nodes: [], edges: [] },
        layout: remote,
        layoutRevision: 8,
      };
    });
  else
    f.read.mockImplementationOnce(async () => {
      await pending.promise;
      return response(remote, 8);
    });
  f.write.mockRejectedValueOnce(stale());
  f.controller.update(local);
  await flush();
  if (operation === 'reload') await f.controller.reload();
  else f.controller.dispose();
  const snapshot = f.controller.snapshot();
  pending.resolve();
  await vi.advanceTimersByTimeAsync(0);
  expect(f.write).toHaveBeenCalledTimes(1);
  if (operation === 'reload') expect(f.controller.snapshot()).toBe(snapshot);
  else expect(f.controller.snapshot()).toMatchObject({ error: true, layout: local });
  f.controller.dispose();
});

it('retains the normal pre-switch final layout flush without consulting a newly selected catalogue', async () => {
  const f = fixture();
  await f.controller.reload();
  f.port.getGraphCatalog = vi.fn().mockResolvedValue({ activeId: 'new' });
  f.controller.update(local);
  f.controller.dispose();
  await vi.advanceTimersByTimeAsync(0);
  expect(f.write).toHaveBeenCalledExactlyOnceWith('g', {
    expectedLayoutRevision: 1,
    layout: local,
  });
  expect(f.port.getGraphCatalog).not.toHaveBeenCalled();
});

it('preserves both projected virtual endpoint positions rather than filtering by native IDs', async () => {
  const f = fixture();
  const positions = { 'io::host-source': { x: 10, y: 20 }, 'io::host-sink': { x: 700, y: 30 } };
  f.read.mockResolvedValueOnce(response({ positions }));
  await f.controller.reload();
  f.port.getGraphCatalog = vi.fn().mockResolvedValue({
    activeId: 'g',
    layoutRevision: 8,
    layout: { positions },
    document: {
      schemaVersion: 2,
      revision: 3,
      edges: [],
      nodes: [
        {
          id: 'io',
          type: 'virtual_endpoint',
          config: {
            displayName: 'MixRelay Virtual I/O',
            reference: { render: true, capture: true },
            render: { channels: 2 },
            capture: { channels: 2 },
          },
        },
      ],
    },
  });
  f.write.mockRejectedValueOnce(stale());
  const updated = { positions: { ...positions, 'io::host-source': { x: 80, y: 20 } } };
  f.controller.update(updated);
  await flush();
  expect(f.write).toHaveBeenLastCalledWith('g', { expectedLayoutRevision: 8, layout: updated });
  f.controller.dispose();
});

it('preserves remote fields during an explicit retry of an unknown acknowledgement', async () => {
  const f = fixture();
  await f.controller.reload();
  f.write.mockRejectedValueOnce(new Error('unknown ack'));
  f.controller.update(local);
  await flush();
  f.read.mockResolvedValueOnce(response(remote, 8));
  await f.controller.retry();
  await vi.advanceTimersByTimeAsync(0);
  expect(f.write).toHaveBeenLastCalledWith('g', { expectedLayoutRevision: 8, layout: merged });
  f.controller.dispose();
});

it('does not retry when a conflict recovery read fails or times out', async () => {
  const f = fixture();
  await f.controller.reload();
  f.write.mockRejectedValueOnce(stale());
  f.read.mockReturnValueOnce(new Promise(() => {}));
  f.controller.update(local);
  await vi.advanceTimersByTimeAsync(10_400);
  expect(f.write).toHaveBeenCalledTimes(1);
  expect(f.controller.snapshot()).toMatchObject({ error: true, saving: false, layout: local });
  f.controller.dispose();
});

it('preserves a fresh reload when an obsolete conflict recovery rejects', async () => {
  const f = fixture();
  await f.controller.reload();
  const oldRead = Promise.withResolvers<GraphLayoutResponse>();
  f.write.mockRejectedValueOnce(stale());
  f.read.mockReturnValueOnce(oldRead.promise);
  f.controller.update(local);
  await flush();
  f.read.mockResolvedValueOnce(response(remote, 8));
  await f.controller.reload();
  const snapshot = f.controller.snapshot();
  oldRead.reject(new Error('old recovery unavailable'));
  await vi.advanceTimersByTimeAsync(0);
  expect(f.controller.snapshot()).toBe(snapshot);
  expect(snapshot).toMatchObject({ layout: remote, error: false, dirty: false, saving: false });
  expect(f.write).toHaveBeenCalledTimes(1);
  f.controller.dispose();
});

it('does not dispatch an obsolete write when saving notification synchronously reloads the layout', async () => {
  const f = fixture();
  await f.controller.reload();
  const unsubscribe = f.controller.subscribe(() => {
    if (f.controller.snapshot().saving) void f.controller.reload();
  });
  f.controller.update(local);
  await flush();
  expect(f.write).not.toHaveBeenCalled();
  expect(f.read).toHaveBeenCalledTimes(2);
  expect(f.controller.snapshot()).toMatchObject({ saving: false, dirty: false, layout: base });
  unsubscribe();
  f.controller.dispose();
});

it.each(['reload', 'dispose'] as const)(
  'does not dispatch the rebased write after a restore notification %s',
  async (operation) => {
    const f = fixture();
    await f.controller.reload();
    f.write.mockRejectedValueOnce(stale());
    f.read.mockResolvedValueOnce(response(remote, 8));
    const unsubscribe = f.controller.subscribe(() => {
      if (f.controller.snapshot().restoreVersion === 2) {
        unsubscribe();
        if (operation === 'reload') void f.controller.reload();
        else f.controller.dispose();
      }
    });
    f.controller.update(local);
    await flush();
    expect(f.write).toHaveBeenCalledTimes(1);
    if (operation === 'reload')
      expect(f.controller.snapshot()).toMatchObject({ dirty: false, layout: base });
    else expect(f.controller.snapshot()).toMatchObject({ error: true, layout: merged });
    f.controller.dispose();
  },
);

it('merges per-instance labels, explicit resets and deleted node IDs independently of geometry', () => {
  const original = { positions: { a: { x: 0, y: 0 } }, nodeLabels: { a: 'Original' } };
  const edited = { ...original, nodeLabels: { a: 'Video' } };
  const remote = { positions: original.positions, nodeLabels: { a: 'Remote', b: 'Meeting' } };
  expect(rebaseGraphLayout(original, edited, remote).nodeLabels).toEqual({
    a: 'Video',
    b: 'Meeting',
  });
  expect(rebaseGraphLayout(original, original, remote).nodeLabels).toEqual(remote.nodeLabels);
  expect(rebaseGraphLayout(original, { ...original, nodeLabels: {} }, remote).nodeLabels).toEqual({
    b: 'Meeting',
  });
  expect(
    rebaseGraphLayout(original, edited, { positions: {}, nodeLabels: { b: 'Meeting' } }).nodeLabels,
  ).toEqual({ a: 'Video', b: 'Meeting' });
  expect(rebaseGraphLayout(original, edited, remote, new Set(['b'])).nodeLabels).toEqual({
    b: 'Meeting',
  });
  expect(
    rebaseGraphLayout({ positions: {} }, { positions: {} }, { positions: {} }).nodeLabels,
  ).toBeUndefined();
});

it('rebases prototype-named UI IDs as ordinary own label entries', () => {
  const base = { positions: {}, nodeLabels: Object.fromEntries([['__proto__', 'Original']]) };
  const local = {
    positions: {},
    nodeLabels: Object.fromEntries([
      ['__proto__', 'Video'],
      ['constructor', 'Meeting'],
    ]),
  };
  expect(rebaseGraphLayout(base, local, base).nodeLabels).toEqual(local.nodeLabels);
  expect(
    rebaseGraphLayout(local, { positions: {}, nodeLabels: {} }, local).nodeLabels,
  ).toBeUndefined();
});
