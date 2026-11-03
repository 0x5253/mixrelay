import { expect, it, vi } from 'vitest';
import { GraphApplicationQueue } from '../../core/application/workspace/GraphApplicationQueue';
import { ControlPortError } from '../../core/domain/control/errors';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import { acknowledgedNodeControl } from '../../core/domain/graph/acknowledgedNodeControl';
import { graphWriterDefaults } from '../../test-support/portDefaults';
import { workspaceRuntime } from '../../test-support/workspaceRuntime';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './model';

const document: RuntimeGraphDocument = {
  schemaVersion: 2,
  revision: 7,
  edges: [],
  nodes: [
    {
      id: 'processor',
      type: 'gain',
      label: 'Gain',
      config: { gainDb: -3 },
      control: { enabled: false, muted: true },
    },
    {
      id: 'missing',
      type: 'unknown_plugin',
      label: null,
      config: { state: 'preserved' },
      control: { enabled: false, muted: false },
    },
  ],
};

it('preserves power and mute for known and unresolved nodes through graph projection', () => {
  expect(audioGraphToRuntimeGraph(runtimeGraphToAudioGraph(document), document)).toEqual(document);
});

function fixture() {
  const restored = { ...document, revision: 8 };
  const completion = Promise.withResolvers<{
    protocolVersion: 1;
    revision: number;
    state: 'active';
    requiresGraphReload: boolean;
  }>();
  let selected: string | undefined = 'graph-a';
  const getGraph = vi.fn().mockResolvedValue(restored);
  const port = {
    ...graphWriterDefaults(getGraph, () => selected),
    updateNodeControl: vi.fn().mockReturnValue(completion.promise),
    applyGraph: vi.fn(),
    getGraph,
    getGraphStatus: vi
      .fn()
      .mockResolvedValue({ activeRevision: 8, submittedRevision: 8, protocolVersion: 1 }),
  };
  const callbacks = {
    getGraphId: () => 'graph-a',
    getDocument: () => document,
    getModel: () => runtimeGraphToAudioGraph(document),
    document: vi.fn(),
    restore: vi.fn(),
    state: vi.fn(),
    error: vi.fn(),
  };
  return {
    select: (graphId: string) => {
      selected = graphId;
    },
    completion,
    port,
    callbacks,
    queue: new GraphApplicationQueue(port, callbacks, workspaceRuntime),
    restored,
  };
}

it('uses the same writer lock and displayed revision without optimistic UI state', async () => {
  const f = fixture();
  const task = f.queue.setNodeControl('graph-a', 'processor', { enabled: true });
  expect(f.port.updateNodeControl).toHaveBeenCalledExactlyOnceWith('graph-a', 'processor', {
    clientOperationId: expect.any(String),
    expectedRevision: 7,
    enabled: true,
  });
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  await expect(f.queue.setNodeControl('graph-a', 'processor', { muted: false })).resolves.toEqual({
    state: 'rejected',
    code: 'graph_exchange_busy',
  });
  expect(f.port.updateNodeControl).toHaveBeenCalledTimes(1);
  f.completion.resolve({
    revision: 8,
    state: 'active',
    protocolVersion: 1,
    requiresGraphReload: false,
  });
  await expect(task).resolves.toEqual({ state: 'applied', revision: 8 });
  expect(f.callbacks.restore).toHaveBeenCalledExactlyOnceWith(
    acknowledgedNodeControl(document, 'processor', { enabled: true }, 8),
  );
  expect(f.port.getGraph).not.toHaveBeenCalled();
  expect(f.port.getGraphStatus).not.toHaveBeenCalled();
  expect(f.port.applyGraph).not.toHaveBeenCalled();
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
});

it('reconciles an external control winner after stale rejection without clearing edit history', async () => {
  const f = fixture();
  const reconcile = vi.fn().mockReturnValue(true);
  const queue = new GraphApplicationQueue(f.port, { ...f.callbacks, reconcile }, workspaceRuntime);
  const task = queue.setNodeControl('graph-a', 'processor', { enabled: true });
  f.completion.reject(new ControlPortError(409, { code: 'graph_revision_stale', parameters: {} }));
  await expect(task).resolves.toEqual({ state: 'rejected', code: 'graph_revision_stale' });
  expect(reconcile).toHaveBeenCalledExactlyOnceWith(document, f.restored);
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
});

it('refreshes controls without treating a new layout model as an audio conflict', async () => {
  const f = fixture();
  const reconcile = vi.fn().mockReturnValue(true);
  const queue = new GraphApplicationQueue(f.port, { ...f.callbacks, reconcile }, workspaceRuntime);
  queue.refresh();
  await vi.waitFor(() => expect(reconcile).toHaveBeenCalledExactlyOnceWith(document, f.restored));
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
});

it('keeps a rejected deleted-node command recoverable without rewriting connections', async () => {
  const f = fixture();
  const task = f.queue.setNodeControl('graph-a', 'processor', { enabled: true });
  f.completion.reject(new ControlPortError(404, { code: 'node_not_found', parameters: {} }));
  await expect(task).resolves.toEqual({ state: 'rejected', code: 'node_not_found' });
  expect(f.callbacks.restore).toHaveBeenCalledExactlyOnceWith(f.restored);
  expect(f.callbacks.error).toHaveBeenLastCalledWith('node_not_found');
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
  expect(f.port.applyGraph).not.toHaveBeenCalled();
  f.port.getGraph.mockResolvedValueOnce({ ...document, revision: 9 });
  f.port.getGraphStatus.mockResolvedValueOnce({
    activeRevision: 9,
    submittedRevision: 9,
    protocolVersion: 1,
  });
  f.port.updateNodeControl.mockResolvedValueOnce({
    revision: 9,
    protocolVersion: 1,
    state: 'active',
    requiresGraphReload: false,
  });
  await f.queue.setNodeControl('graph-a', 'missing', { muted: true });
  expect(f.port.updateNodeControl).toHaveBeenLastCalledWith('graph-a', 'missing', {
    clientOperationId: expect.any(String),
    expectedRevision: 8,
    muted: true,
  });
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
});

it('does not restore a foreign graph when selection changes before acknowledgement', async () => {
  const f = fixture();
  let graphId = 'graph-a';
  const queue = new GraphApplicationQueue(
    f.port,
    { ...f.callbacks, getGraphId: () => graphId },
    workspaceRuntime,
  );
  const task = queue.setNodeControl('graph-a', 'processor', { enabled: true }, 7);
  graphId = 'graph-b';
  f.completion.resolve({
    revision: 8,
    state: 'active',
    protocolVersion: 1,
    requiresGraphReload: false,
  });
  await expect(task).resolves.toMatchObject({ state: 'uncertain' });
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.callbacks.document).not.toHaveBeenCalled();
});

it('accepts an acknowledged no-op without rereading or incrementing the revision', async () => {
  const f = fixture();
  const task = f.queue.setNodeControl('graph-a', 'processor', { enabled: false });
  f.completion.resolve({
    revision: 7,
    state: 'active',
    protocolVersion: 1,
    requiresGraphReload: false,
  });
  await expect(task).resolves.toEqual({ state: 'applied', revision: 7 });
  expect(f.port.getGraph).not.toHaveBeenCalled();
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it('rereads canonical state after native preparation instead of losing other reconciled metadata', async () => {
  const f = fixture();
  const reconcile = vi.fn().mockReturnValue(false);
  const queue = new GraphApplicationQueue(f.port, { ...f.callbacks, reconcile }, workspaceRuntime);
  const canonical = {
    ...f.restored,
    nodes: f.restored.nodes.map((node) => ({
      ...node,
      config: { ...(node.config as object), nativeMetadata: 'updated by preparation' },
    })),
  };
  f.port.getGraph.mockResolvedValueOnce(canonical);
  const task = queue.setNodeControl('graph-a', 'processor', { enabled: true });
  f.completion.resolve({
    revision: 8,
    state: 'active',
    protocolVersion: 1,
    requiresGraphReload: true,
  });
  await expect(task).resolves.toEqual({ state: 'applied', revision: 8 });
  expect(f.port.getGraph).toHaveBeenCalledTimes(1);
  expect(f.callbacks.restore).toHaveBeenCalledExactlyOnceWith(canonical);
  expect(f.callbacks.document).toHaveBeenLastCalledWith(canonical);
  expect(reconcile).not.toHaveBeenCalled();
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it('does not report an unrelated acknowledgement revision as applied', async () => {
  const f = fixture();
  const task = f.queue.setNodeControl('graph-a', 'processor', { enabled: true }, 7);
  f.completion.resolve({
    revision: 9,
    state: 'active',
    protocolVersion: 1,
    requiresGraphReload: false,
  });
  await expect(task).resolves.toEqual({ state: 'uncertain', code: 'graph_revision_stale' });
  expect(f.callbacks.restore).not.toHaveBeenCalled();
});

it('does not apply a late acknowledgement after the workspace is replaced', async () => {
  const f = fixture();
  const task = f.queue.setNodeControl('graph-a', 'processor', { enabled: true });
  f.queue.reset();
  f.completion.resolve({
    revision: 8,
    state: 'active',
    protocolVersion: 1,
    requiresGraphReload: false,
  });
  await expect(task).resolves.toMatchObject({ state: 'uncertain' });
  expect(f.callbacks.restore).not.toHaveBeenCalled();
});

it('rejects foreign graph identity and caller revision before dispatch', async () => {
  const f = fixture();
  for (const [graphId, revision] of [
    ['graph-b', 7],
    ['graph-a', 6],
  ] as const) {
    await expect(
      f.queue.setNodeControl(graphId, 'processor', { enabled: true }, revision),
    ).resolves.toEqual({ state: 'rejected', code: 'graph_revision_stale' });
  }
  expect(f.port.updateNodeControl).not.toHaveBeenCalled();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
});

it('reports lost replies as uncertain and does not replay the command', async () => {
  const f = fixture();
  f.port.updateNodeControl.mockRejectedValue(new Error('connection lost'));
  await expect(
    f.queue.setNodeControl('graph-a', 'processor', { enabled: true }, 7),
  ).resolves.toEqual({ state: 'uncertain', code: 'control_request_failed' });
  expect(f.port.updateNodeControl).toHaveBeenCalledTimes(1);
});

it('does not retarget queued power intent to a replacement node with the same ID', async () => {
  const f = fixture();
  let current = document;
  const pending = Promise.withResolvers<void>();
  const queue = new GraphApplicationQueue(
    f.port,
    {
      ...f.callbacks,
      beforeApply: () => pending.promise,
      getDocument: () => current,
    },
    workspaceRuntime,
  );
  const task = queue.setNodeControl('graph-a', 'processor', { enabled: true });
  current = { ...document, revision: 9 };
  pending.resolve();
  await task;
  expect(f.port.updateNodeControl).not.toHaveBeenCalled();
  expect(f.callbacks.error).toHaveBeenLastCalledWith(null);
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
});

it('preserves a concurrently edited draft after a rejected node command', async () => {
  const f = fixture();
  const task = f.queue.setNodeControl('graph-a', 'processor', { enabled: true });
  f.queue.submit(runtimeGraphToAudioGraph({ ...document, revision: 9 }));
  f.completion.reject(new ControlPortError(409, { code: 'node_disabled', parameters: {} }));
  await task;
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it.each([true, false])(
  'uses the authoritative rejection marker (%s), not a 503 alone, to keep controls usable',
  async (definite) => {
    const f = fixture();
    const task = f.queue.setNodeControl('graph-a', 'processor', { enabled: true });
    f.completion.reject(
      new ControlPortError(503, {
        code: 'control_unavailable',
        parameters: definite ? { nodeControlOutcome: 'not_applied' } : {},
      }),
    );
    await task;
    expect(f.callbacks.state).toHaveBeenLastCalledWith(definite ? 'active' : 'blocked');
    expect(f.callbacks.restore).toHaveBeenCalledTimes(definite ? 1 : 0);
    expect(f.callbacks.error).toHaveBeenLastCalledWith('control_unavailable');
    f.port.updateNodeControl.mockResolvedValueOnce({
      revision: 8,
      protocolVersion: 1,
      state: 'active',
      requiresGraphReload: false,
    });
    await f.queue.setNodeControl('graph-a', 'missing', { muted: true });
    expect(f.port.updateNodeControl).toHaveBeenCalledTimes(definite ? 2 : 1);
    if (definite)
      expect(f.port.updateNodeControl).toHaveBeenLastCalledWith('graph-a', 'missing', {
        clientOperationId: expect.any(String),
        expectedRevision: 8,
        muted: true,
      });
    expect(f.port.applyGraph).not.toHaveBeenCalled();
  },
);

it.each(['engine_replaced', 'invalid_operation_id'] as const)(
  'treats an engine fence (%s) as not admitted: reloads controls and reports it',
  async (code) => {
    const f = fixture();
    const task = f.queue.setNodeControl('graph-a', 'processor', { enabled: true });
    f.completion.reject(
      new ControlPortError(code === 'engine_replaced' ? 409 : 400, {
        code,
        parameters: { operationOutcome: 'not_admitted' },
      }),
    );
    await expect(task).resolves.toEqual({ state: 'rejected', code });
    expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
    expect(f.callbacks.restore).toHaveBeenCalledTimes(1);
    expect(f.callbacks.error).toHaveBeenLastCalledWith(code);
    expect(f.port.applyGraph).not.toHaveBeenCalled();
  },
);
