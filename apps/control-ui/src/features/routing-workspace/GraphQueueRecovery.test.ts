import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { GraphApplicationQueue } from '../../core/application/workspace/GraphApplicationQueue';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { GraphLayoutResponse } from '../../core/domain/control/catalog';
import { ControlPortError } from '../../core/domain/control/errors';
import type {
  GraphApplyRequest,
  GraphApplyResponse,
  NodeControlResponse,
  RuntimeGraphDocument,
} from '../../core/domain/control/graph';
import { graphWriterDefaults } from '../../test-support/portDefaults';
import { workspaceRuntime } from '../../test-support/workspaceRuntime';
import { runtimeGraphToAudioGraph } from './model';

const source: RuntimeGraphDocument = {
  schemaVersion: 2,
  revision: 4,
  edges: [],
  nodes: [{ id: 'gain', type: 'gain', label: 'Gain', config: { gainDb: 0 } }],
};
const retryTarget = {
  expectedCatalogRevision: 7,
  expectedInstanceId: 'failed-instance',
  graphId: 'graph-a',
  nodeId: 'effect',
};
const effectDocument: RuntimeGraphDocument = {
  ...source,
  nodes: [{ id: 'effect', type: 'provider', label: 'Effect', config: { typeId: 'effect' } }],
};

function fixture() {
  let document: RuntimeGraphDocument | null = source;
  let model: AudioGraphModel | null = runtimeGraphToAudioGraph(source);
  let graphId = 'graph-a';
  const getGraph = vi.fn(async () => document ?? source);
  const port = {
    ...graphWriterDefaults(getGraph, () => graphId),
    applyGraph: vi.fn(
      async (request: GraphApplyRequest): Promise<GraphApplyResponse> => ({
        protocolVersion: 1,
        revision: request.revision,
        state: 'active',
      }),
    ),
    getGraph,
    getGraphStatus: vi.fn(async () => ({
      activeRevision: document?.revision ?? source.revision,
      submittedRevision: document?.revision ?? source.revision,
      protocolVersion: 1 as const,
    })),
    updateNodeControl: vi.fn(
      async (): Promise<NodeControlResponse> => ({
        protocolVersion: 1,
        revision: source.revision + 1,
        state: 'active',
        requiresGraphReload: false,
      }),
    ),
  };
  const callbacks = {
    getDocument: () => document,
    getModel: () => model,
    getGraphId: () => graphId,
    beforeApply: vi.fn(async () => {}),
    getReloadLayout: vi.fn(
      async (): Promise<GraphLayoutResponse> => ({
        graphId,
        layoutRevision: 2,
        layout: { positions: { gain: { x: 30, y: 40 } } },
      }),
    ),
    document: vi.fn((next: RuntimeGraphDocument) => {
      document = next;
    }),
    restore: vi.fn(),
    reconcile: vi.fn(() => true),
    state: vi.fn(),
    error: vi.fn(),
  };
  return {
    port,
    callbacks,
    queue: new GraphApplicationQueue(port, callbacks, workspaceRuntime),
    setDocument: (next: RuntimeGraphDocument | null) => {
      document = next;
    },
    setModel: (next: AudioGraphModel | null) => {
      model = next;
    },
    setGraphId: (next: string) => {
      graphId = next;
    },
  };
}

const flush = () => vi.advanceTimersByTimeAsync(0);
beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

it('rejects a command for a deleted node before sending it', async () => {
  const f = fixture();
  f.setDocument(null);
  await expect(f.queue.setNodeControl('graph-a', 'gain', { muted: true })).resolves.toEqual({
    state: 'rejected',
    code: 'node_not_found',
  });
  expect(f.port.updateNodeControl).not.toHaveBeenCalled();
});

it('rejects a command authored against another revision', async () => {
  const f = fixture();
  await expect(f.queue.setNodeControl('graph-a', 'gain', { muted: true }, 3)).resolves.toEqual({
    state: 'rejected',
    code: 'graph_revision_stale',
  });
  expect(f.port.updateNodeControl).not.toHaveBeenCalled();
});

it('restores native metadata when the control acknowledgement requests a reload', async () => {
  const f = fixture();
  const next = { ...source, revision: 5 };
  f.port.updateNodeControl.mockResolvedValue({
    protocolVersion: 1,
    revision: 5,
    state: 'active',
    requiresGraphReload: true,
  });
  f.port.getGraph.mockResolvedValue(next);
  f.port.getGraphStatus.mockResolvedValue({
    protocolVersion: 1,
    activeRevision: 5,
    submittedRevision: 5,
  });
  await expect(f.queue.setNodeControl('graph-a', 'gain', { muted: true })).resolves.toEqual({
    state: 'applied',
    revision: 5,
  });
  expect(f.callbacks.restore).toHaveBeenCalledExactlyOnceWith(next);
  expect(f.callbacks.reconcile).not.toHaveBeenCalled();
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it.each(['malformed reload flag', 'unrelated revision'])(
  'blocks an uncertain acknowledgement with %s rather than replaying it',
  async (problem) => {
    const f = fixture();
    const response = {
      protocolVersion: 1,
      revision: problem === 'unrelated revision' ? 12 : 5,
      state: 'active',
      requiresGraphReload: problem === 'malformed reload flag' ? undefined : false,
    } as unknown as NodeControlResponse;
    f.port.updateNodeControl.mockResolvedValue(response);
    const result = await f.queue.setNodeControl('graph-a', 'gain', { muted: true });
    expect(result).toEqual({
      state: 'uncertain',
      code: problem === 'unrelated revision' ? 'graph_revision_stale' : 'control_request_failed',
    });
    expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
    expect(f.port.updateNodeControl).toHaveBeenCalledTimes(1);
    expect(f.port.applyGraph).not.toHaveBeenCalled();
  },
);

it('waits for a pending control acknowledgement before publishing its state', async () => {
  const f = fixture();
  f.port.updateNodeControl.mockResolvedValue({
    protocolVersion: 1,
    revision: 5,
    state: 'pending',
    requiresGraphReload: false,
  });
  f.port.getGraphStatus.mockResolvedValue({
    protocolVersion: 1,
    activeRevision: 5,
    submittedRevision: 5,
  });
  await expect(f.queue.setNodeControl('graph-a', 'gain', { muted: true })).resolves.toEqual({
    state: 'applied',
    revision: 5,
  });
  expect(f.callbacks.reconcile).toHaveBeenCalledWith(
    source,
    expect.objectContaining({ revision: 5 }),
  );
});

it('keeps a queued user edit when a node-control response arrives', async () => {
  const f = fixture();
  const response = Promise.withResolvers<NodeControlResponse>();
  f.port.updateNodeControl.mockReturnValue(response.promise);
  const command = f.queue.setNodeControl('graph-a', 'gain', { muted: true });
  await flush();
  f.queue.submit(runtimeGraphToAudioGraph({ ...source, revision: 20 }));
  response.resolve({
    protocolVersion: 1,
    revision: 5,
    state: 'active',
    requiresGraphReload: false,
  });
  await expect(command).resolves.toEqual({ state: 'uncertain', code: 'graph_draft_changed' });
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it('does not publish controls after reconciliation detects a changed topology', async () => {
  const f = fixture();
  f.callbacks.reconcile.mockReturnValue(false);
  await expect(f.queue.setNodeControl('graph-a', 'gain', { muted: true })).resolves.toEqual({
    state: 'uncertain',
    code: 'graph_revision_stale',
  });
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
  expect(f.port.updateNodeControl).toHaveBeenCalledTimes(1);
});

it('rechecks the baseline after settling parameters and rejects a concurrent writer', async () => {
  const f = fixture();
  f.callbacks.beforeApply.mockImplementation(async () => {
    f.setDocument({ ...source, revision: 8 });
  });
  await expect(f.queue.setNodeControl('graph-a', 'gain', { muted: true })).resolves.toEqual({
    state: 'rejected',
    code: 'graph_revision_stale',
  });
  expect(f.port.updateNodeControl).not.toHaveBeenCalled();
});

it('does not attach an old graph failure to the newly selected graph', async () => {
  const f = fixture();
  const response = Promise.withResolvers<NodeControlResponse>();
  f.port.updateNodeControl.mockReturnValue(response.promise);
  const command = f.queue.setNodeControl('graph-a', 'gain', { muted: true });
  await flush();
  f.setGraphId('graph-b');
  response.reject(new Error('connection lost'));
  await expect(command).resolves.toEqual({ state: 'uncertain', code: 'control_request_failed' });
  expect(f.callbacks.error).toHaveBeenCalledExactlyOnceWith(null);
  expect(f.port.getGraph).not.toHaveBeenCalled();
});

it('cancels an in-flight control and leaves the reset queue available', async () => {
  const f = fixture();
  const response = Promise.withResolvers<NodeControlResponse>();
  f.port.updateNodeControl.mockReturnValue(response.promise);
  const command = f.queue.setNodeControl('graph-a', 'gain', { muted: true });
  await flush();
  f.queue.reset(source);
  await expect(command).resolves.toMatchObject({ state: 'uncertain' });
  response.resolve({
    protocolVersion: 1,
    revision: 5,
    state: 'active',
    requiresGraphReload: false,
  });
  await flush();
  expect(f.callbacks.document).not.toHaveBeenCalled();
  expect(f.queue.canPublishParameters).toBe(true);
  expect(vi.getTimerCount()).toBe(0);
});

it('falls back to restoring controls when no reconciliation callback is available', async () => {
  const f = fixture();
  const queue = new GraphApplicationQueue(
    f.port,
    { ...f.callbacks, reconcile: undefined },
    workspaceRuntime,
  );
  await expect(queue.setNodeControl('graph-a', 'gain', { muted: true })).resolves.toMatchObject({
    state: 'applied',
  });
  expect(f.callbacks.restore).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({ revision: 5 }),
  );
});

it('reloads saved layout only for the selected graph', async () => {
  const f = fixture();
  f.queue.reload();
  await flush();
  expect(f.callbacks.getReloadLayout).toHaveBeenCalledExactlyOnceWith('graph-a');
  expect(f.callbacks.restore).toHaveBeenCalledExactlyOnceWith(source, {
    positions: { gain: { x: 30, y: 40 } },
  });
});

it('rejects a saved layout whose graph identity does not match', async () => {
  const f = fixture();
  f.callbacks.getReloadLayout.mockResolvedValue({
    graphId: 'graph-b',
    layoutRevision: 2,
    layout: { positions: { gain: { x: 0, y: 0 } } },
  });
  f.queue.reload();
  await flush();
  expect(f.callbacks.error).toHaveBeenLastCalledWith('graph_layout_identity_mismatch');
  expect(f.callbacks.restore).not.toHaveBeenCalled();
});

it('does not restore a late layout after the user selects another graph', async () => {
  const f = fixture();
  const layout = Promise.withResolvers<Awaited<ReturnType<typeof f.callbacks.getReloadLayout>>>();
  f.callbacks.getReloadLayout.mockReturnValue(layout.promise);
  f.queue.reload();
  await flush();
  f.setGraphId('graph-b');
  f.callbacks.error.mockClear();
  f.callbacks.state.mockClear();
  layout.resolve({ graphId: 'graph-a', layoutRevision: 2, layout: { positions: {} } });
  await flush();
  expect(f.callbacks.error).not.toHaveBeenCalled();
  expect(f.callbacks.state).not.toHaveBeenCalled();
  expect(f.callbacks.document).not.toHaveBeenCalled();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it('refreshes acknowledged controls while preserving the editable layout', async () => {
  const f = fixture();
  f.queue.refresh();
  await flush();
  expect(f.callbacks.reconcile).toHaveBeenCalledExactlyOnceWith(source, source);
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it('uses the authoritative document when the workspace has no earlier baseline', async () => {
  const f = fixture();
  f.setDocument(null);
  f.queue.refresh();
  await flush();
  expect(f.callbacks.error).toHaveBeenLastCalledWith(null);
  expect(f.callbacks.restore).toHaveBeenLastCalledWith(source);
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
});

it('blocks writes without a source and leaves the editor model intact', async () => {
  const f = fixture();
  f.setDocument(null);
  f.queue.submit(runtimeGraphToAudioGraph(source));
  await flush();
  expect(f.callbacks.error).toHaveBeenLastCalledWith('control_request_failed');
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it('retains a blocked draft when recovery cannot read the engine', async () => {
  const f = fixture();
  f.port.applyGraph.mockRejectedValue(new Error('offline'));
  f.port.getGraph.mockRejectedValue(new Error('offline'));
  f.queue.submit(runtimeGraphToAudioGraph(source));
  await flush();
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.queue.canPublishParameters).toBe(false);
});

it('never adopts another graph as the recovery baseline after a remote selection', async () => {
  const f = fixture();
  const other = { ...source, revision: 9, nodes: [] };
  f.port.applyGraph.mockRejectedValue(
    new ControlPortError(409, { code: 'graph_revision_stale', parameters: {} }),
  );
  f.port.getGraph.mockResolvedValue(other);
  const queue = new GraphApplicationQueue(
    {
      ...f.port,
      getGraphCatalog: vi.fn(async () => ({
        protocolVersion: 1,
        catalogRevision: 8,
        activeId: 'graph-b',
        graphs: [],
        document: other,
      })),
    },
    f.callbacks,
    workspaceRuntime,
  );
  queue.submit(runtimeGraphToAudioGraph(source));
  await flush();
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
  expect(f.callbacks.document).not.toHaveBeenCalledWith(other);
  expect(f.callbacks.restore).not.toHaveBeenCalled();
});

it('takes the recovery baseline from the catalog snapshot that names the owning graph', async () => {
  const f = fixture();
  const other = { ...source, revision: 9, nodes: [] };
  const owned = { ...source, revision: 8 };
  f.port.applyGraph.mockRejectedValue(
    new ControlPortError(409, { code: 'graph_revision_stale', parameters: {} }),
  );
  // A selection has already published B's document while the catalog still names A.
  f.port.getGraph.mockResolvedValue(other);
  f.port.getGraphStatus.mockResolvedValue({
    protocolVersion: 1,
    activeRevision: 8,
    submittedRevision: 8,
  });
  const getGraphCatalog = vi.fn(async () => ({
    protocolVersion: 1,
    catalogRevision: 7,
    activeId: 'graph-a',
    graphs: [],
    document: owned,
  }));
  const queue = new GraphApplicationQueue(
    { ...f.port, getGraphCatalog },
    f.callbacks,
    workspaceRuntime,
  );
  queue.submit(runtimeGraphToAudioGraph(source));
  await flush();
  expect(f.port.getGraph).not.toHaveBeenCalled();
  expect(f.callbacks.document).not.toHaveBeenCalledWith(other);
  expect(f.callbacks.restore).toHaveBeenCalledWith(owned);
});

it('rejects an unrelated apply receipt without silently accepting another writer', async () => {
  const f = fixture();
  f.port.applyGraph.mockResolvedValue({ protocolVersion: 1, revision: 99, state: 'active' });
  f.queue.submit(runtimeGraphToAudioGraph(source));
  await flush();
  expect(f.callbacks.error).toHaveBeenLastCalledWith('graph_revision_stale');
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
});

it('settles a definite remote rejection when recovery is cancelled', async () => {
  const f = fixture();
  const recovery = Promise.withResolvers<RuntimeGraphDocument>();
  f.port.updateNodeControl.mockRejectedValue(
    new ControlPortError(409, { code: 'node_disabled', parameters: {} }),
  );
  f.port.getGraph.mockReturnValue(recovery.promise);
  const command = f.queue.setNodeControl('graph-a', 'gain', { muted: true });
  await flush();
  f.queue.reset(source);
  await expect(command).resolves.toEqual({ state: 'rejected', code: 'node_disabled' });
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.callbacks.error).toHaveBeenCalledExactlyOnceWith(null);
  expect(vi.getTimerCount()).toBe(0);
});

it('rejects a processor retry without a source document and sends no write', async () => {
  const f = fixture();
  const retryProcessor = vi.fn();
  const queue = new GraphApplicationQueue(
    { ...f.port, retryProcessor },
    f.callbacks,
    workspaceRuntime,
  );
  f.setDocument(null);
  await expect(queue.retryFailedProcessor(retryTarget)).resolves.toEqual({
    state: 'rejected',
    code: 'node_not_found',
  });
  expect(retryProcessor).not.toHaveBeenCalled();
});

it('does not retry a processor against a stale catalog snapshot', async () => {
  const f = fixture();
  f.setDocument(effectDocument);
  const retryProcessor = vi.fn();
  const queue = new GraphApplicationQueue(
    { ...f.port, retryProcessor },
    {
      ...f.callbacks,
      getCatalogRevision: () => 9,
    },
    workspaceRuntime,
  );
  await expect(queue.retryFailedProcessor(retryTarget)).resolves.toEqual({
    state: 'rejected',
    code: 'graph_revision_stale',
  });
  expect(retryProcessor).not.toHaveBeenCalled();
});

it('revalidates a processor source after parameter settlement before retry', async () => {
  const f = fixture();
  f.setDocument(effectDocument);
  f.callbacks.beforeApply.mockImplementation(async () => {
    f.setDocument({ ...effectDocument, revision: 9 });
  });
  const retryProcessor = vi.fn();
  const queue = new GraphApplicationQueue(
    { ...f.port, retryProcessor },
    f.callbacks,
    workspaceRuntime,
  );
  await expect(queue.retryFailedProcessor(retryTarget)).resolves.toEqual({
    state: 'rejected',
    code: 'graph_revision_stale',
  });
  expect(retryProcessor).not.toHaveBeenCalled();
});

it('does not claim a failed instance is retryable after the runtime replaces it', async () => {
  const f = fixture();
  f.setDocument(effectDocument);
  const retryProcessor = vi.fn();
  const queue = new GraphApplicationQueue(
    { ...f.port, retryProcessor },
    {
      ...f.callbacks,
      hasFailedProcessor: () => false,
    },
    workspaceRuntime,
  );
  await expect(queue.retryFailedProcessor(retryTarget)).resolves.toEqual({
    state: 'rejected',
    code: 'processor_instance_stale',
  });
  expect(retryProcessor).not.toHaveBeenCalled();
});

it('keeps a retry uncertain when its acknowledgement names an unrelated revision', async () => {
  const f = fixture();
  f.setDocument(effectDocument);
  const retryProcessor = vi.fn(
    async (): Promise<GraphApplyResponse> => ({
      protocolVersion: 1,
      state: 'active',
      revision: 50,
    }),
  );
  const queue = new GraphApplicationQueue(
    { ...f.port, retryProcessor },
    {
      ...f.callbacks,
      beforeApply: undefined,
    },
    workspaceRuntime,
  );
  await expect(queue.retryFailedProcessor(retryTarget)).resolves.toEqual({
    state: 'uncertain',
    code: 'graph_revision_stale',
  });
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(retryProcessor).toHaveBeenCalledTimes(1);
});

it('waits for a pending retry without requiring optional catalog or persistence callbacks', async () => {
  const f = fixture();
  f.setDocument(effectDocument);
  f.port.getGraphStatus.mockResolvedValue({
    protocolVersion: 1,
    activeRevision: 5,
    submittedRevision: 5,
  });
  const retryProcessor = vi.fn(
    async (): Promise<GraphApplyResponse> => ({
      protocolVersion: 1,
      state: 'pending',
      revision: 5,
    }),
  );
  const queue = new GraphApplicationQueue(
    { ...f.port, retryProcessor },
    {
      ...f.callbacks,
      beforeApply: undefined,
    },
    workspaceRuntime,
  );
  await expect(queue.retryFailedProcessor(retryTarget)).resolves.toEqual({
    state: 'applied',
    revision: 5,
  });
  expect(f.callbacks.document).toHaveBeenCalledExactlyOnceWith({ ...effectDocument, revision: 5 });
  expect(f.callbacks.restore).not.toHaveBeenCalled();
});

it('rejects a late retry acknowledgement after the catalog advances beyond the retry', async () => {
  const f = fixture();
  f.setDocument(effectDocument);
  let catalogRevision = retryTarget.expectedCatalogRevision;
  const response = Promise.withResolvers<GraphApplyResponse>();
  const retryProcessor = vi.fn(() => response.promise);
  const queue = new GraphApplicationQueue(
    { ...f.port, retryProcessor },
    {
      ...f.callbacks,
      getCatalogRevision: () => catalogRevision,
    },
    workspaceRuntime,
  );
  const command = queue.retryFailedProcessor(retryTarget);
  await flush();
  catalogRevision += 2;
  response.resolve({ protocolVersion: 1, state: 'active', revision: 5 });
  await expect(command).resolves.toEqual({ state: 'uncertain', code: 'graph_revision_stale' });
  expect(f.callbacks.restore).not.toHaveBeenCalled();
});

it('does not overwrite a queued graph edit with a successful processor retry', async () => {
  const f = fixture();
  f.setDocument(effectDocument);
  const response = Promise.withResolvers<GraphApplyResponse>();
  const retryProcessor = vi.fn(() => response.promise);
  const queue = new GraphApplicationQueue(
    { ...f.port, retryProcessor },
    f.callbacks,
    workspaceRuntime,
  );
  const command = queue.retryFailedProcessor(retryTarget);
  await flush();
  queue.submit(runtimeGraphToAudioGraph({ ...effectDocument, revision: 20 }));
  response.resolve({ protocolVersion: 1, state: 'active', revision: 5 });
  await expect(command).resolves.toEqual({ state: 'uncertain', code: 'graph_draft_changed' });
  expect(f.port.applyGraph).not.toHaveBeenCalled();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
});

it('settles cancellation that happens before parameter settlement returns its promise', async () => {
  const f = fixture();
  f.callbacks.beforeApply.mockImplementation(async () => {
    f.queue.reset(source);
  });
  await expect(f.queue.setNodeControl('graph-a', 'gain', { muted: true })).resolves.toMatchObject({
    state: 'rejected',
  });
  expect(f.port.updateNodeControl).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});

it('ignores retry, reload and refresh while another write owns the graph', async () => {
  const f = fixture();
  const response = Promise.withResolvers<GraphApplyResponse>();
  f.port.applyGraph.mockReturnValue(response.promise);
  f.queue.submit(runtimeGraphToAudioGraph(source));
  await flush();
  f.queue.retry();
  f.queue.reload();
  f.queue.refresh();
  expect(f.port.getGraph).not.toHaveBeenCalled();
  f.queue.reset(source);
  response.resolve({ protocolVersion: 1, state: 'active', revision: 5 });
  await flush();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
});

it('preserves an edit queued while a native control reload is reading its acknowledgement', async () => {
  const f = fixture();
  f.port.updateNodeControl.mockResolvedValue({
    protocolVersion: 1,
    revision: 5,
    state: 'active',
    requiresGraphReload: true,
  });
  f.port.getGraph.mockImplementation(async () => {
    f.queue.submit(runtimeGraphToAudioGraph({ ...source, revision: 9 }));
    return { ...source, revision: 5 };
  });
  f.port.getGraphStatus.mockResolvedValue({
    protocolVersion: 1,
    activeRevision: 5,
    submittedRevision: 5,
  });
  await expect(f.queue.setNodeControl('graph-a', 'gain', { muted: true })).resolves.toEqual({
    state: 'uncertain',
    code: 'graph_draft_changed',
  });
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it('expires a pending node control whose engine never acknowledges its revision', async () => {
  const f = fixture();
  f.port.updateNodeControl.mockResolvedValue({
    protocolVersion: 1,
    revision: 5,
    state: 'pending',
    requiresGraphReload: false,
  });
  const command = f.queue.setNodeControl('graph-a', 'gain', { muted: true });
  await vi.advanceTimersByTimeAsync(10_000);
  await expect(command).resolves.toEqual({ state: 'uncertain', code: 'graph_apply_timeout' });
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
  expect(f.callbacks.restore).not.toHaveBeenCalled();
});

it('discards cancellation during a rejected control recovery read', async () => {
  const f = fixture();
  f.port.updateNodeControl.mockRejectedValue(
    new ControlPortError(404, { code: 'node_not_found', parameters: {} }),
  );
  f.port.getGraph.mockImplementation(async () => {
    f.queue.reset(source);
    throw new Error('old read ended');
  });
  await expect(f.queue.setNodeControl('graph-a', 'gain', { muted: true })).resolves.toEqual({
    state: 'rejected',
    code: 'node_not_found',
  });
  expect(f.callbacks.state).not.toHaveBeenCalledWith('blocked');
  expect(f.callbacks.restore).not.toHaveBeenCalled();
});

it.each(['read-failure', 'document-change', 'graph-selection'] as const)(
  'does not publish a retired recovery result after %s',
  async (change) => {
    const f = fixture();
    f.port.updateNodeControl.mockRejectedValue(new Error('transport disconnected'));
    if (change === 'read-failure') {
      f.port.getGraph.mockImplementation(async () => {
        f.queue.reset(source);
        throw new Error('owner retired');
      });
    } else {
      f.callbacks.document.mockImplementation(() => {
        if (change === 'document-change') f.queue.reset(source);
        else f.setGraphId('graph-b');
      });
    }
    await expect(f.queue.setNodeControl('graph-a', 'gain', { muted: true })).resolves.toMatchObject(
      { state: 'uncertain' },
    );
    expect(f.callbacks.state).not.toHaveBeenCalledWith('blocked');
    expect(f.callbacks.error).not.toHaveBeenCalledWith('control_request_failed');
  },
);

it.each(['wrong-revision', 'different-graph'] as const)(
  'rejects a native reload acknowledgement for %s',
  async (mismatch) => {
    const f = fixture();
    f.port.updateNodeControl.mockResolvedValue({
      protocolVersion: 1,
      revision: 5,
      state: 'active',
      requiresGraphReload: true,
    });
    if (mismatch === 'different-graph')
      f.port.getGraph.mockImplementation(async () => {
        f.setGraphId('graph-b');
        return source;
      });
    await expect(f.queue.setNodeControl('graph-a', 'gain', { muted: true })).resolves.toEqual({
      state: 'uncertain',
      code: 'graph_revision_stale',
    });
    expect(f.callbacks.restore).not.toHaveBeenCalled();
  },
);

it('acknowledges node controls without a parameter-settlement callback', async () => {
  const f = fixture();
  const queue = new GraphApplicationQueue(
    f.port,
    { ...f.callbacks, beforeApply: undefined },
    workspaceRuntime,
  );
  await expect(queue.setNodeControl('graph-a', 'gain', { muted: true })).resolves.toEqual({
    state: 'applied',
    revision: 5,
  });
  expect(f.callbacks.reconcile).toHaveBeenCalledOnce();
});

it('publishes a separate persistence warning without repeating an acknowledged node command', async () => {
  const f = fixture();
  const persistenceWarning = vi.fn();
  const queue = new GraphApplicationQueue(
    f.port,
    { ...f.callbacks, persistenceWarning },
    workspaceRuntime,
  );
  f.port.updateNodeControl.mockResolvedValueOnce({
    protocolVersion: 1,
    revision: 5,
    state: 'active',
    requiresGraphReload: false,
    persistenceError: 'graph_catalog_unavailable',
  });
  await expect(queue.setNodeControl('graph-a', 'gain', { muted: true })).resolves.toMatchObject({
    state: 'applied',
  });
  expect(persistenceWarning).toHaveBeenLastCalledWith('graph_catalog_unavailable');
  f.port.updateNodeControl.mockResolvedValueOnce({
    protocolVersion: 1,
    revision: 6,
    state: 'active',
    requiresGraphReload: false,
  });
  await expect(queue.setNodeControl('graph-a', 'gain', { muted: false })).resolves.toMatchObject({
    state: 'applied',
  });
  expect(persistenceWarning).toHaveBeenLastCalledWith(null);
  expect(f.port.updateNodeControl).toHaveBeenCalledTimes(2);
});

it('blocks a rejected control when its recovery read fails without cancellation', async () => {
  const f = fixture();
  f.port.updateNodeControl.mockRejectedValue(
    new ControlPortError(404, { code: 'node_not_found', parameters: {} }),
  );
  f.port.getGraph.mockRejectedValue(new Error('engine offline'));
  await expect(f.queue.setNodeControl('graph-a', 'gain', { muted: true })).resolves.toMatchObject({
    state: 'rejected',
  });
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
});

it('rejects a retry for another graph and supports a current graph without a settlement hook', async () => {
  const f = fixture();
  f.setDocument(effectDocument);
  f.setModel(runtimeGraphToAudioGraph(effectDocument));
  const retryProcessor = vi.fn(
    async (): Promise<GraphApplyResponse> => ({ protocolVersion: 1, revision: 5, state: 'active' }),
  );
  const queue = new GraphApplicationQueue(
    { ...f.port, retryProcessor },
    { ...f.callbacks, beforeApply: undefined },
    workspaceRuntime,
  );
  await expect(
    queue.retryFailedProcessor({ ...retryTarget, graphId: 'graph-b' }),
  ).resolves.toMatchObject({ state: 'rejected', code: 'graph_revision_stale' });
  expect(retryProcessor).not.toHaveBeenCalled();
  await expect(queue.retryFailedProcessor(retryTarget)).resolves.toMatchObject({
    state: 'applied',
  });
});

it('does not announce an old writer as active when its acknowledgement retires the view', async () => {
  const f = fixture();
  f.callbacks.document.mockImplementation(() => f.queue.reset(source));
  f.queue.submit(runtimeGraphToAudioGraph(source));
  await flush();
  expect(f.callbacks.state).not.toHaveBeenCalledWith('active');
  expect(f.port.applyGraph).toHaveBeenCalledOnce();
});

it('discards a late processor retry reply after its writer is reset', async () => {
  const f = fixture();
  f.setDocument(effectDocument);
  const response = Promise.withResolvers<GraphApplyResponse>();
  const retryProcessor = vi.fn(() => response.promise);
  const queue = new GraphApplicationQueue(
    { ...f.port, retryProcessor },
    f.callbacks,
    workspaceRuntime,
  );
  const command = queue.retryFailedProcessor(retryTarget);
  await flush();
  expect(retryProcessor).toHaveBeenCalledOnce();
  queue.reset(effectDocument);
  response.resolve({ protocolVersion: 1, revision: 5, state: 'active' });
  await expect(command).resolves.toMatchObject({ state: 'uncertain' });
  expect(f.callbacks.document).not.toHaveBeenCalled();
  expect(f.callbacks.state).not.toHaveBeenCalledWith('active');
});
