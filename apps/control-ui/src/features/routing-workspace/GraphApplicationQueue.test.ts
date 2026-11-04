import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import {
  GRAPH_APPLICATION_TIMEOUT_MS,
  GraphApplicationQueue,
} from '../../core/application/workspace/GraphApplicationQueue';
import { ControlPortError } from '../../core/domain/control/errors';
import type {
  GraphApplyRequest,
  GraphApplyResponse,
  RuntimeGraphDocument,
} from '../../core/domain/control/graph';
import { controlState, statePush } from '../../test-support/controlTelemetry';
import { graphWriterDefaults } from '../../test-support/portDefaults';
import { workspaceRuntime } from '../../test-support/workspaceRuntime';
import { addRoutingNode } from './routingNodeCreation';
import { runtimeGraphToAudioGraph } from './runtimeGraphProjection';
import { virtualSpec } from './virtualEndpointFixtures';

const empty: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
function fixture(beforeReload?: () => Promise<void>) {
  let draft = addRoutingNode(
    runtimeGraphToAudioGraph(empty),
    { kind: 'virtual_input', spec: virtualSpec },
    { x: 20, y: 30 },
  );
  let selected: string | undefined;
  const getGraph = vi.fn().mockResolvedValue(empty);
  const port = {
    ...graphWriterDefaults(getGraph, () => selected),
    isOperationPending: vi.fn().mockResolvedValue(false),
    getGraph,
    getGraphStatus: vi
      .fn()
      .mockResolvedValue({ activeRevision: 1, submittedRevision: 1, protocolVersion: 1 }),
    applyGraph: vi.fn(
      async (document: GraphApplyRequest): Promise<GraphApplyResponse> => ({
        protocolVersion: 1,
        revision: document.revision,
        state: 'active',
      }),
    ),
  };
  const callbacks = {
    beforeReload,
    getDocument: () => empty,
    getModel: () => draft,
    document: vi.fn(),
    restore: vi.fn(),
    state: vi.fn(),
    error: vi.fn(),
    persistenceWarning: vi.fn(),
    permissionWaiting: vi.fn(),
  };
  const queue = new GraphApplicationQueue(port, callbacks, workspaceRuntime);
  return {
    select: (graphId: string) => {
      selected = graphId;
    },
    queue,
    port,
    callbacks,
    get draft() {
      return draft;
    },
    edit: () => {
      draft = { ...draft, revision: draft.revision + 10 };
      return draft;
    },
  };
}
const flush = () => vi.advanceTimersByTimeAsync(0);
beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

it('confirms pending graph application through the shared pushed revision', async () => {
  const f = fixture();
  const push = statePush(controlState(1, 1));
  Object.assign(f.port, push);
  f.port.applyGraph.mockResolvedValueOnce({ protocolVersion: 1, revision: 2, state: 'pending' });
  f.port.getGraph.mockResolvedValueOnce({ ...empty, revision: 2 });
  f.queue.submit(f.draft);
  await flush();
  expect(f.port.getGraphStatus).not.toHaveBeenCalled();
  push.push(controlState(2, 2));
  await flush();
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
  f.queue.reset();
});

it('warns about failed persistence without retrying or blocking an applied graph', async () => {
  const f = fixture();
  f.port.applyGraph.mockResolvedValueOnce({
    protocolVersion: 1,
    revision: 2,
    state: 'active',
    persistenceError: 'graph_catalog_save_failed',
  });
  f.queue.submit(f.draft);
  await flush();
  expect(f.callbacks.persistenceWarning).toHaveBeenLastCalledWith('graph_catalog_save_failed');
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
  expect(f.callbacks.error).toHaveBeenLastCalledWith(null);
  expect(f.queue.canPublishParameters).toBe(true);
  expect(f.port.applyGraph).toHaveBeenCalledTimes(1);
  expect(f.port.getGraph).not.toHaveBeenCalled();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  f.queue.submit(f.edit());
  await flush();
  expect(f.port.applyGraph).toHaveBeenCalledTimes(2);
  expect(f.port.applyGraph.mock.calls[1]?.[0].revision).toBe(3);
  expect(f.callbacks.persistenceWarning).toHaveBeenLastCalledWith(null);
});

it('does not clear a persistence warning until a later graph is acknowledged', async () => {
  const f = fixture();
  f.port.applyGraph.mockResolvedValueOnce({
    protocolVersion: 1,
    revision: 2,
    state: 'active',
    persistenceError: 'graph_catalog_save_failed',
  });
  f.queue.submit(f.draft);
  await flush();
  f.port.applyGraph.mockRejectedValueOnce(new Error('offline'));
  f.queue.submit(f.edit());
  await flush();
  expect(f.callbacks.persistenceWarning).toHaveBeenCalledTimes(1);
  expect(f.callbacks.persistenceWarning).toHaveBeenLastCalledWith('graph_catalog_save_failed');
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
});

it('serializes edits and coalesces only the queued snapshot, using engine revisions', async () => {
  const f = fixture();
  let accept!: (response: GraphApplyResponse) => void;
  f.port.applyGraph.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        accept = resolve;
      }),
  );
  f.queue.submit(f.draft);
  f.queue.submit(f.edit());
  const last = f.edit();
  f.queue.submit(last);
  expect(f.port.applyGraph).toHaveBeenCalledTimes(1);
  accept({ protocolVersion: 1, revision: 2, state: 'active' });
  await flush();
  expect(f.port.applyGraph.mock.calls.map(([doc]) => doc.revision)).toEqual([2, 3]);
  expect(f.port.applyGraph.mock.calls[1]?.[0].nodes).toHaveLength(1);
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
  expect(vi.getTimerCount()).toBe(0);
});

it('adopts a newer authoritative graph after a definite conflict and permits the next edit', async () => {
  const f = fixture();
  f.port.applyGraph.mockRejectedValueOnce(
    new ControlPortError(409, { code: 'graph_revision_stale', parameters: {} }),
  );
  const current = { ...empty, revision: 8 };
  f.port.getGraph.mockResolvedValue(current);
  f.port.getGraphStatus.mockResolvedValue({
    activeRevision: 8,
    submittedRevision: 8,
    protocolVersion: 1,
  });
  f.queue.submit(f.draft);
  await flush();
  expect(f.callbacks.restore).toHaveBeenLastCalledWith(current);
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
  expect(f.callbacks.error).toHaveBeenLastCalledWith(null);
  expect(f.port.applyGraph).toHaveBeenCalledTimes(1);
  expect(f.queue.canPublishParameters).toBe(true);
  f.queue.submit(f.edit());
  await flush();
  expect(f.port.applyGraph.mock.calls[1]?.[0].revision).toBe(9);
});

it('keeps an edit submitted while a definite conflict reads the engine document', async () => {
  const f = fixture();
  f.port.applyGraph.mockRejectedValueOnce(
    new ControlPortError(409, { code: 'graph_revision_stale', parameters: {} }),
  );
  const current = { ...empty, revision: 8 };
  const read = Promise.withResolvers<RuntimeGraphDocument>();
  f.port.getGraph.mockReturnValueOnce(read.promise).mockResolvedValue(current);
  f.port.getGraphStatus.mockResolvedValue({
    activeRevision: 8,
    submittedRevision: 8,
    protocolVersion: 1,
  });
  f.queue.submit(f.draft);
  await flush();
  f.queue.submit(f.edit());
  read.resolve(current);
  await flush();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
  expect(f.callbacks.error).toHaveBeenLastCalledWith('graph_draft_changed');
  expect(f.port.applyGraph).toHaveBeenCalledTimes(1);
  // An explicit retry writes the kept draft over the engine document it read.
  f.queue.retry();
  await flush();
  expect(f.port.applyGraph).toHaveBeenCalledTimes(2);
  expect(f.port.applyGraph.mock.calls[1]?.[0]).toMatchObject({ revision: 9, nodes: [{}] });
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
});

it('does not treat a newer active graph as acknowledgement of our edit', async () => {
  const f = fixture();
  f.port.applyGraph.mockResolvedValue({ protocolVersion: 1, revision: 2, state: 'pending' });
  f.port.getGraphStatus.mockResolvedValue({
    activeRevision: 3,
    submittedRevision: 3,
    protocolVersion: 1,
  });
  f.queue.submit(f.draft);
  await flush();
  expect(f.callbacks.error).toHaveBeenLastCalledWith('graph_revision_stale');
  expect(f.callbacks.state).not.toHaveBeenCalledWith('active');
});

it.each(['pending', 'stalled request', 'stalled status'] as const)(
  'bounds %s and leaves no polling timers',
  async (mode) => {
    const f = fixture();
    f.port.applyGraph.mockResolvedValue({ protocolVersion: 1, revision: 2, state: 'pending' });
    if (mode === 'stalled request')
      f.port.applyGraph.mockImplementation(() => new Promise(() => {}));
    if (mode === 'stalled status')
      f.port.getGraphStatus.mockImplementation(() => new Promise(() => {}));
    f.queue.submit(f.draft);
    await vi.advanceTimersByTimeAsync(GRAPH_APPLICATION_TIMEOUT_MS + 200);
    expect(f.callbacks.error).toHaveBeenLastCalledWith('graph_apply_timeout');
    expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
    expect(f.callbacks.restore).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  },
);

it('cancels a waiting poll and ignores a late reply after disposal', async () => {
  const f = fixture();
  let accept!: (response: GraphApplyResponse) => void;
  f.port.applyGraph.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        accept = resolve;
      }),
  );
  f.queue.submit(f.draft);
  f.queue.reset();
  accept({ protocolVersion: 1, revision: 2, state: 'active' });
  await flush();
  expect(f.callbacks.document).not.toHaveBeenCalled();
  expect(f.callbacks.state).not.toHaveBeenCalledWith('active');
  expect(vi.getTimerCount()).toBe(0);
});

it('cancels a scheduled activation poll, settling its wait', async () => {
  const f = fixture();
  f.port.applyGraph.mockResolvedValue({ protocolVersion: 1, revision: 2, state: 'pending' });
  f.queue.submit(f.draft);
  await flush();
  const polls = f.port.getGraphStatus.mock.calls.length;
  f.queue.reset();
  await vi.advanceTimersByTimeAsync(GRAPH_APPLICATION_TIMEOUT_MS * 2);
  expect(f.port.getGraphStatus).toHaveBeenCalledTimes(polls);
  expect(f.callbacks.error).toHaveBeenCalledTimes(1); // only the initial null
  expect(vi.getTimerCount()).toBe(0);
});

it('reloads the actual engine document without submitting a write', async () => {
  const f = fixture();
  f.queue.reload();
  await flush();
  expect(f.callbacks.restore).toHaveBeenCalledExactlyOnceWith(empty, undefined);
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it('waits for parameter settlement before reading a replacement engine document', async () => {
  const settlement = Promise.withResolvers<void>();
  const f = fixture(() => settlement.promise);
  f.queue.reload();
  await flush();
  expect(f.port.getGraph).not.toHaveBeenCalled();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  settlement.resolve();
  await flush();
  expect(f.port.getGraph).toHaveBeenCalledTimes(1);
  expect(f.callbacks.restore).toHaveBeenCalledExactlyOnceWith(empty, undefined);
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it('does not discard edits made while a reload is in flight', async () => {
  const f = fixture();
  let resolve!: (document: RuntimeGraphDocument) => void;
  f.port.getGraph.mockImplementationOnce(
    () =>
      new Promise((accept) => {
        resolve = accept;
      }),
  );
  f.queue.reload();
  f.queue.submit(f.edit());
  resolve(empty);
  await flush();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.port.applyGraph).not.toHaveBeenCalled();
  expect(f.callbacks.error).toHaveBeenLastCalledWith('graph_draft_changed');
});

it('follows the accepted first-use operation through consent and applies approval without replay', async () => {
  const f = fixture();
  f.port.isOperationPending.mockResolvedValue(true);
  f.port.getGraphStatus.mockResolvedValue({
    protocolVersion: 1,
    activeRevision: 1,
    submittedRevision: 1,
    pendingReason: 'awaiting_microphone_permission',
  });
  const consent = Promise.withResolvers<GraphApplyResponse>();
  f.port.applyGraph.mockReturnValueOnce(consent.promise);
  f.queue.submit(f.draft);
  await vi.advanceTimersByTimeAsync(60_000);
  expect(f.callbacks.state).toHaveBeenLastCalledWith('applying');
  expect(f.callbacks.permissionWaiting).toHaveBeenLastCalledWith(true);
  expect(f.callbacks.document).not.toHaveBeenCalled();
  expect(f.callbacks.restore).not.toHaveBeenCalled();
  expect(f.port.applyGraph).toHaveBeenCalledTimes(1);
  expect(f.port.isOperationPending).toHaveBeenCalledWith(
    f.port.applyGraph.mock.calls[0]?.[0].clientOperationId,
  );
  consent.resolve({ protocolVersion: 1, revision: 2, state: 'active' });
  await flush();
  expect(f.callbacks.state).toHaveBeenLastCalledWith('active');
  expect(f.callbacks.document).toHaveBeenCalledWith(expect.objectContaining({ revision: 2 }));
  expect(f.port.applyGraph).toHaveBeenCalledTimes(1);
  expect(f.callbacks.error).toHaveBeenLastCalledWith(null);
});
