import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { GraphApplicationQueue } from '../../core/application/workspace/GraphApplicationQueue';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import { ControlPortError } from '../../core/domain/control/errors';
import type { GraphApplyResponse, RuntimeGraphDocument } from '../../core/domain/control/graph';
import { rebaseGraphEdit } from '../../core/domain/graph/rebaseGraphEdit';
import { graphWriterDefaults } from '../../test-support/portDefaults';
import { workspaceRuntime } from '../../test-support/workspaceRuntime';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';

const base: RuntimeGraphDocument = {
  schemaVersion: 2,
  revision: 1,
  nodes: [
    {
      id: 'comp',
      type: 'compressor',
      label: null,
      config: {
        version: 1,
        channels: 2,
        bypass: false,
        thresholdDb: -18,
        ratio: 4,
        attackMs: 10,
        releaseMs: 100,
        makeupGainDb: 0,
      },
    },
  ],
  edges: [],
};
const rejection = () => new ControlPortError(409, { code: 'graph_revision_stale', parameters: {} });
function fixture() {
  let document = base;
  let model = runtimeGraphToAudioGraph(base);
  let selected: string | undefined;
  const getGraph = vi.fn(async () => document);
  const port = {
    ...graphWriterDefaults(getGraph, () => selected),
    getGraph,
    getGraphStatus: vi.fn(async () => ({
      protocolVersion: 1 as const,
      activeRevision: document.revision,
      submittedRevision: document.revision,
    })),
    applyGraph: vi.fn(
      async (request: RuntimeGraphDocument): Promise<GraphApplyResponse> => ({
        protocolVersion: 1,
        state: 'active',
        revision: request.revision,
      }),
    ),
  };
  const callbacks = {
    getDocument: () => document,
    getModel: () => model,
    document: vi.fn((value: RuntimeGraphDocument) => {
      document = value;
    }),
    restore: vi.fn((value: RuntimeGraphDocument) => {
      model = runtimeGraphToAudioGraph(value);
    }),
    reconcile: vi.fn(() => false),
    rebase: vi.fn(
      (before: RuntimeGraphDocument, current: RuntimeGraphDocument, local: AudioGraphModel) => {
        model = runtimeGraphToAudioGraph(
          rebaseGraphEdit(before, audioGraphToRuntimeGraph(local, before), current),
        );
        return model;
      },
    ),
    state: vi.fn(),
    error: vi.fn(),
  };
  const queue = new GraphApplicationQueue(port, callbacks, workspaceRuntime);
  return {
    select: (graphId: string) => {
      selected = graphId;
    },
    port,
    callbacks,
    queue,
    remote: (next: RuntimeGraphDocument) => {
      document = next;
    },
    model: () => model,
  };
}
beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());
const flush = () => vi.advanceTimersByTimeAsync(0);
it('automatically adopts external parameter/topology changes and leaves controls actionable without a write', async () => {
  const f = fixture();
  const current = { ...base, revision: 2, nodes: [] };
  f.remote(current);
  f.queue.reset(base);
  f.queue.refresh();
  await flush();
  expect(f.callbacks.restore).toHaveBeenLastCalledWith(current);
  expect(f.callbacks.error).toHaveBeenLastCalledWith(null);
  expect(f.queue.canPublishParameters).toBe(true);
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});
it('rebases an edit queued during a refresh onto the fresh settings', async () => {
  const f = fixture();
  const read = Promise.withResolvers<RuntimeGraphDocument>();
  f.port.getGraph.mockReturnValueOnce(read.promise);
  f.queue.refresh();
  const local = {
    ...base,
    nodes: base.nodes.map((n) => ({ ...n, config: { ...(n.config as object), ratio: 8 } })),
  };
  f.queue.submit(runtimeGraphToAudioGraph(local));
  const current = {
    ...base,
    revision: 2,
    nodes: base.nodes.map((n) => ({ ...n, config: { ...(n.config as object), thresholdDb: -30 } })),
  };
  f.remote(current);
  read.resolve(current);
  await flush();
  expect(f.port.applyGraph.mock.calls[0]?.[0]).toMatchObject({
    revision: 3,
    nodes: [{ config: { ratio: 8, thresholdDb: -30 } }],
  });
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
});
it('retries a definitely rejected edit once using current values and coalesces newer user input', async () => {
  const f = fixture();
  const response = Promise.withResolvers<GraphApplyResponse>();
  f.port.applyGraph.mockReturnValueOnce(response.promise);
  const local = (ratio: number) =>
    runtimeGraphToAudioGraph({
      ...base,
      nodes: base.nodes.map((n) => ({ ...n, config: { ...(n.config as object), ratio } })),
    });
  f.queue.submit(local(8));
  f.queue.submit(local(12));
  const current = {
    ...base,
    revision: 5,
    nodes: base.nodes.map((n) => ({ ...n, config: { ...(n.config as object), thresholdDb: -30 } })),
  };
  f.remote(current);
  response.reject(rejection());
  await flush();
  expect(f.port.applyGraph).toHaveBeenCalledTimes(2);
  expect(f.port.applyGraph.mock.calls[1]?.[0]).toMatchObject({
    revision: 6,
    nodes: [{ config: { ratio: 12, thresholdDb: -30 } }],
  });
  expect(f.queue.canPublishParameters).toBe(true);
});
it('preserves local intent across two ordinary concurrent rejections without exposing recovery choices', async () => {
  const f = fixture();
  f.port.applyGraph.mockRejectedValueOnce(rejection()).mockRejectedValueOnce(rejection());
  f.queue.reset(base);
  f.remote({
    ...base,
    revision: 5,
    nodes: base.nodes.map((node) => ({
      ...node,
      config: { ...(node.config as object), thresholdDb: -30 },
    })),
  });
  f.queue.submit(
    runtimeGraphToAudioGraph({
      ...base,
      nodes: base.nodes.map((node) => ({
        ...node,
        config: { ...(node.config as object), ratio: 8 },
      })),
    }),
  );
  await vi.advanceTimersByTimeAsync(200);
  expect(f.port.applyGraph).toHaveBeenCalledTimes(3);
  expect(f.port.applyGraph).toHaveBeenLastCalledWith(
    expect.objectContaining({
      revision: 6,
      nodes: [
        expect.objectContaining({
          config: expect.objectContaining({ ratio: 8, thresholdDb: -30 }),
        }),
      ],
    }),
  );
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.callbacks.state).not.toHaveBeenCalledWith('blocked');
  expect(f.callbacks.error).not.toHaveBeenCalledWith('graph_revision_stale');
  expect(f.queue.canPublishParameters).toBe(true);
});

it('preserves the rebased draft and offers recovery only after continuous CAS contention exceeds its deadline', async () => {
  const f = fixture();
  f.queue.reset(base);
  f.port.applyGraph.mockRejectedValue(rejection());
  const local = runtimeGraphToAudioGraph({
    ...base,
    nodes: base.nodes.map((node) => ({
      ...node,
      config: { ...(node.config as object), ratio: 8 },
    })),
  });
  f.queue.submit(local);
  await vi.advanceTimersByTimeAsync(10_000);
  expect(f.callbacks.error).toHaveBeenLastCalledWith('graph_conflict_unresolved');
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.model().nodes[0]?.runtime?.config).toMatchObject({ ratio: 8 });
  expect(f.queue.canPublishParameters).toBe(false);
  expect(vi.getTimerCount()).toBe(0);
});
it('never replays a lost acknowledgement and preserves explicit parameter intent for genuine failure recovery', async () => {
  const f = fixture();
  f.port.applyGraph.mockRejectedValueOnce(new Error('lost reply'));
  f.queue.submit(f.model());
  await flush();
  expect(f.port.applyGraph).toHaveBeenCalledTimes(1);
  expect(f.callbacks.rebase).not.toHaveBeenCalled();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
});

it('rebases an explicit retry against the failed edit baseline instead of overwriting unrelated external settings', async () => {
  const f = fixture();
  const local = runtimeGraphToAudioGraph({
    ...base,
    nodes: base.nodes.map((node) => ({
      ...node,
      config: { ...(node.config as object), ratio: 8 },
    })),
  });
  const current = {
    ...base,
    revision: 5,
    nodes: base.nodes.map((node) => ({
      ...node,
      config: { ...(node.config as object), thresholdDb: -30 },
    })),
  };
  f.port.applyGraph.mockRejectedValueOnce(new Error('lost reply'));
  f.queue.reset(base);
  f.queue.submit(local);
  f.remote(current);
  f.port.getGraph.mockResolvedValue(current);
  await flush();
  expect(f.port.applyGraph).toHaveBeenCalledTimes(1);
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  f.callbacks.getModel = () => local;
  f.queue.retry();
  await flush();
  expect(f.port.applyGraph).toHaveBeenCalledTimes(2);
  expect(f.port.applyGraph).toHaveBeenLastCalledWith(
    expect.objectContaining({
      revision: 6,
      nodes: [
        expect.objectContaining({
          config: expect.objectContaining({ ratio: 8, thresholdDb: -30 }),
        }),
      ],
    }),
  );
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
});

it('keeps a pending rebased edit when reading after its definite rejection also reports a stale revision', async () => {
  const f = fixture();
  f.queue.reset(base);
  f.port.applyGraph.mockRejectedValueOnce(rejection());
  f.port.getGraph.mockRejectedValueOnce(rejection());
  f.queue.submit(f.model());
  await flush();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
  expect(f.port.applyGraph).toHaveBeenCalledTimes(1);
});

it.each([
  new Error('lost rebased reply'),
  new ControlPortError(503, { code: 'control_request_failed', parameters: {} }),
])(
  'does not automatically repeat a rebased write with an uncertain acknowledgement: %s',
  async (error) => {
    const f = fixture();
    f.queue.reset(base);
    f.port.applyGraph.mockRejectedValueOnce(rejection()).mockRejectedValueOnce(error);
    f.queue.submit(f.model());
    await vi.advanceTimersByTimeAsync(1000);
    expect(f.port.applyGraph).toHaveBeenCalledTimes(2);
    expect(f.callbacks.restore).not.toHaveBeenCalled();
    expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
    expect(vi.getTimerCount()).toBe(0);
  },
);

it('cancels automatic CAS retry while waiting for its backoff without replaying a stale draft', async () => {
  const f = fixture();
  f.port.applyGraph.mockRejectedValue(rejection());
  f.queue.submit(f.model());
  await flush();
  expect(f.port.applyGraph).toHaveBeenCalledTimes(2);
  f.queue.reset(base);
  await vi.advanceTimersByTimeAsync(1000);
  expect(f.port.applyGraph).toHaveBeenCalledTimes(2);
  expect(f.callbacks.state).not.toHaveBeenCalledWith('blocked');
  expect(vi.getTimerCount()).toBe(0);
});

it('preserves a new writer edit when explicit retry rebasing retires the failed writer', async () => {
  const f = fixture();
  f.port.applyGraph.mockRejectedValueOnce(new Error('lost reply'));
  f.queue.submit(f.model());
  await flush();
  const next = runtimeGraphToAudioGraph({ ...base, nodes: [] });
  f.callbacks.rebase.mockImplementation(() => {
    f.queue.reset(base);
    f.queue.submit(next);
    return runtimeGraphToAudioGraph(base);
  });
  f.queue.retry();
  await flush();
  expect(f.port.applyGraph).toHaveBeenCalledTimes(2);
  expect(f.port.applyGraph).toHaveBeenLastCalledWith(expect.objectContaining({ nodes: [] }));
});
it('does not publish a late conflict read after cancellation', async () => {
  const f = fixture();
  const read = Promise.withResolvers<RuntimeGraphDocument>();
  f.port.applyGraph.mockRejectedValueOnce(rejection());
  f.port.getGraph.mockReturnValueOnce(read.promise);
  f.queue.submit(f.model());
  await flush();
  f.queue.reset();
  read.resolve({ ...base, revision: 9 });
  await flush();
  expect(f.port.applyGraph).toHaveBeenCalledTimes(1);
  expect(f.callbacks.rebase).not.toHaveBeenCalled();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});

it('keeps the active graph identity when reconciling two definite conflicts', async () => {
  const f = fixture();
  f.select('current-graph');
  const callbacks = { ...f.callbacks, getGraphId: () => 'current-graph' };
  const queue = new GraphApplicationQueue(f.port, callbacks, workspaceRuntime);
  queue.reset(base);
  f.remote({ ...base, revision: 4 });
  f.port.applyGraph.mockRejectedValueOnce(rejection()).mockRejectedValueOnce(rejection());
  queue.submit(f.model());
  await vi.advanceTimersByTimeAsync(200);
  expect(f.port.applyGraph).toHaveBeenCalledTimes(3);
  expect(callbacks.restore).not.toHaveBeenCalled();
  expect(callbacks.state).toHaveBeenLastCalledWith('active');
});
it('preserves the draft if reading current state after a definite rejection fails', async () => {
  const f = fixture();
  const callbacks = { ...f.callbacks, rebase: undefined };
  const queue = new GraphApplicationQueue(f.port, callbacks, workspaceRuntime);
  f.port.applyGraph.mockRejectedValueOnce(rejection());
  f.port.getGraph.mockRejectedValueOnce(new Error('offline'));
  queue.submit(f.model());
  await flush();
  expect(callbacks.restore).not.toHaveBeenCalled();
  expect(callbacks.state).toHaveBeenLastCalledWith('blocked');
  expect(f.port.applyGraph).toHaveBeenCalledTimes(1);
});
it('cancels a definite-conflict recovery read without setting a blocked state', async () => {
  const f = fixture();
  const read = Promise.withResolvers<RuntimeGraphDocument>();
  const callbacks = { ...f.callbacks, rebase: undefined };
  const queue = new GraphApplicationQueue(f.port, callbacks, workspaceRuntime);
  f.port.applyGraph.mockRejectedValueOnce(rejection());
  f.port.getGraph.mockReturnValueOnce(read.promise);
  queue.submit(f.model());
  await flush();
  queue.reset();
  read.resolve({ ...base, revision: 5 });
  await flush();
  expect(callbacks.state).not.toHaveBeenCalledWith('blocked');
  expect(callbacks.restore).not.toHaveBeenCalled();
});
it('cancels an in-flight rebased write without replaying it or blocking subsequent work', async () => {
  const f = fixture();
  const response = Promise.withResolvers<GraphApplyResponse>();
  f.queue.reset(base);
  f.remote({ ...base, revision: 4 });
  f.port.applyGraph.mockRejectedValueOnce(rejection()).mockReturnValueOnce(response.promise);
  f.queue.submit(f.model());
  await flush();
  expect(f.port.applyGraph).toHaveBeenCalledTimes(2);
  f.queue.reset();
  response.reject(new Error('late reply'));
  await flush();
  expect(f.callbacks.state).not.toHaveBeenCalledWith('blocked');
  expect(vi.getTimerCount()).toBe(0);
});
it('blocks a refresh with a concurrent draft if the consumer has no rebase contract', async () => {
  const f = fixture();
  const read = Promise.withResolvers<RuntimeGraphDocument>();
  const queue = new GraphApplicationQueue(
    f.port,
    { ...f.callbacks, rebase: undefined },
    workspaceRuntime,
  );
  f.port.getGraph.mockReturnValueOnce(read.promise);
  queue.refresh();
  queue.submit(f.model());
  read.resolve({ ...base, revision: 3 });
  await flush();
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
  expect(f.callbacks.error).toHaveBeenLastCalledWith('graph_draft_changed');
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it('does not retain or replay a cancelled initial graph write', async () => {
  const f = fixture();
  const response = Promise.withResolvers<GraphApplyResponse>();
  f.port.applyGraph.mockReturnValueOnce(response.promise);
  f.queue.submit(f.model());
  f.queue.reset();
  response.reject(new Error('late reply'));
  await flush();
  expect(f.callbacks.state).not.toHaveBeenCalledWith('blocked');
  expect(f.port.applyGraph).toHaveBeenCalledTimes(1);
  expect(vi.getTimerCount()).toBe(0);
});

it('adopts current settings after an accepted node command loses its exact reload baseline', async () => {
  const f = fixture();
  f.remote({ ...base, revision: 3 });
  const native = {
    ...f.port,
    updateNodeControl: vi.fn().mockResolvedValue({
      protocolVersion: 1,
      state: 'active',
      revision: 2,
      requiresGraphReload: true,
    }),
  };
  f.select('current-graph');
  const callbacks = { ...f.callbacks, getGraphId: () => 'current-graph' };
  native.getGraph.mockRejectedValueOnce(rejection());
  const queue = new GraphApplicationQueue(native, callbacks, workspaceRuntime);
  queue.reset(base);
  const command = queue.setNodeControl('current-graph', 'comp', { enabled: false });
  await flush();
  expect(await command).toEqual({ state: 'uncertain', code: 'graph_revision_stale' });
  expect(callbacks.restore).toHaveBeenLastCalledWith({ ...base, revision: 3 });
  expect(callbacks.state).toHaveBeenLastCalledWith('active');
  expect(native.updateNodeControl).toHaveBeenCalledTimes(1);
});

it('does not restore a refresh after its document publication synchronously retires the writer', async () => {
  const f = fixture();
  f.callbacks.document.mockImplementation(() => f.queue.reset());
  f.queue.refresh();
  await vi.advanceTimersByTimeAsync(0);
  expect(f.callbacks.document).toHaveBeenCalledTimes(1);
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.callbacks.reconcile).not.toHaveBeenCalled();
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it('does not fall back to old restore when reconciliation retires the refresh writer', async () => {
  const f = fixture();
  f.queue.reset(base);
  f.callbacks.reconcile.mockImplementation(() => {
    f.queue.reset();
    return false;
  });
  f.queue.refresh();
  await flush();
  expect(f.callbacks.reconcile).toHaveBeenCalledTimes(1);
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.callbacks.state).toHaveBeenLastCalledWith('applying');
});

it.each(['refresh', 'rejection'] as const)(
  'preserves the next writer pending edit when %s rebase retires the old owner',
  async (mode) => {
    const f = fixture();
    f.queue.reset(base);
    const changed = (ratio: number) =>
      runtimeGraphToAudioGraph({
        ...base,
        nodes: base.nodes.map((node) => ({
          ...node,
          config: { ...(node.config as object), ratio },
        })),
      });
    f.callbacks.rebase.mockImplementation(() => {
      f.queue.reset(base);
      f.queue.submit(changed(3));
      return changed(8);
    });
    if (mode === 'refresh') {
      const pending = Promise.withResolvers<RuntimeGraphDocument>();
      f.port.getGraph.mockReturnValueOnce(pending.promise);
      f.queue.refresh();
      f.queue.submit(changed(8));
      pending.resolve(base);
    } else {
      f.port.applyGraph.mockRejectedValueOnce(rejection());
      f.queue.submit(changed(8));
    }
    await flush();
    expect(f.port.applyGraph).toHaveBeenCalledTimes(mode === 'refresh' ? 1 : 2);
    expect(f.port.applyGraph).toHaveBeenLastCalledWith(
      expect.objectContaining({
        nodes: [expect.objectContaining({ config: expect.objectContaining({ ratio: 3 }) })],
      }),
    );
  },
);

it('does not publish old recovery state after the conflict restore callback retires the writer', async () => {
  const f = fixture();
  f.port.applyGraph.mockRejectedValue(rejection());
  f.callbacks.restore.mockImplementation(() => f.queue.reset());
  const queue = new GraphApplicationQueue(
    f.port,
    { ...f.callbacks, rebase: undefined },
    workspaceRuntime,
  );
  f.callbacks.restore.mockImplementation(() => queue.reset());
  queue.submit(f.model());
  await flush();
  expect(f.callbacks.restore).toHaveBeenCalledTimes(1);
  expect(f.callbacks.state).toHaveBeenLastCalledWith('applying');
  expect(f.callbacks.error).toHaveBeenCalledTimes(1);
});

it('does not read or restore a newly selected graph when publication changes identity without resetting', async () => {
  const f = fixture();
  let graphId = 'old';
  f.select('old');
  const callbacks = { ...f.callbacks, getGraphId: () => graphId };
  callbacks.document.mockImplementation(() => {
    graphId = 'new';
  });
  const queue = new GraphApplicationQueue(f.port, callbacks, workspaceRuntime);
  queue.reset(base);
  queue.refresh();
  await flush();
  // The selected graph's document is read once, through the catalog that names it.
  expect(f.port.getGraphCatalog).toHaveBeenCalledTimes(1);
  expect(callbacks.restore).not.toHaveBeenCalled();
  expect(callbacks.reconcile).not.toHaveBeenCalled();
  expect(callbacks.state).toHaveBeenLastCalledWith('applying');
});

it('does not publish a blocked state when recovery publication retires its writer in the next microtask', async () => {
  const f = fixture();
  f.port.applyGraph.mockRejectedValueOnce(new Error('lost acknowledgement'));
  f.callbacks.document.mockImplementation(() => queueMicrotask(() => f.queue.reset()));
  f.queue.submit(f.model());
  await flush();
  expect(f.callbacks.document).toHaveBeenCalledTimes(1);
  expect(f.callbacks.state).not.toHaveBeenCalledWith('blocked');
  expect(f.callbacks.error).toHaveBeenCalledExactlyOnceWith(null);
  expect(f.queue.canPublishParameters).toBe(true);
  expect(vi.getTimerCount()).toBe(0);
});

it('does not report an old write failure into a newly selected graph when recovery read also fails', async () => {
  const f = fixture();
  let graphId = 'old';
  const read = Promise.withResolvers<RuntimeGraphDocument>();
  f.port.applyGraph.mockRejectedValueOnce(new Error('lost acknowledgement'));
  f.port.getGraph.mockReturnValueOnce(read.promise);
  const callbacks = { ...f.callbacks, getGraphId: () => graphId };
  const queue = new GraphApplicationQueue(f.port, callbacks, workspaceRuntime);
  queue.submit(f.model());
  await flush();
  expect(f.port.getGraph).toHaveBeenCalledTimes(1);
  graphId = 'new';
  read.reject(new Error('old graph unavailable'));
  await flush();
  expect(callbacks.state).not.toHaveBeenCalledWith('blocked');
  expect(callbacks.error).toHaveBeenCalledExactlyOnceWith(null);
  expect(callbacks.restore).not.toHaveBeenCalled();
  expect(callbacks.document).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});

it.each([false, true])(
  'does not republish node control acknowledgement when its restore/reconcile retires the owner (reload=%s)',
  async (requiresGraphReload) => {
    const f = fixture();
    const native = {
      ...f.port,
      updateNodeControl: vi.fn().mockResolvedValue({
        protocolVersion: 1,
        state: 'active',
        revision: 2,
        requiresGraphReload,
      }),
    };
    f.select('g');
    const callbacks = { ...f.callbacks, getGraphId: () => 'g' };
    const queue = new GraphApplicationQueue(native, callbacks, workspaceRuntime);
    queue.reset(base);
    f.remote({ ...base, revision: 2 });
    callbacks.reconcile.mockImplementation(() => {
      queue.reset();
      return true;
    });
    callbacks.restore.mockImplementation(() => queue.reset());
    const result = await queue.setNodeControl('g', 'comp', { enabled: false });
    expect(result.state).toBe('uncertain');
    expect(callbacks.document).toHaveBeenCalledTimes(requiresGraphReload ? 1 : 0);
    expect(callbacks.state).toHaveBeenLastCalledWith('applying');
  },
);
