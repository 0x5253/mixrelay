import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { GraphApplicationQueue } from '../../core/application/workspace/GraphApplicationQueue';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type {
  GraphApplyRequest,
  GraphApplyResponse,
  ProcessorParameterChange,
  RuntimeGraphDocument,
} from '../../core/domain/control/graph';
import { graphWriterDefaults } from '../../test-support/portDefaults';
import { workspaceRuntime } from '../../test-support/workspaceRuntime';
import { changedProcessorParameters, updateRoutingNodeParameter } from './model';

const change = (parameterId: number, value: number): ProcessorParameterChange => ({
  nodeId: 'p',
  typeId: 'vst3:test',
  parameterId,
  value,
});
function fixture(beforeApply?: () => Promise<void>) {
  const config = {
    typeId: 'vst3:test',
    parameters: {
      stateRef: { sha256: 'a'.repeat(64), byteLength: 8 },
      parameterValues: { '1': 0.1, '2': 0.2 },
    },
  };
  let draft: AudioGraphModel = {
    revision: 1,
    edges: [],
    nodes: [
      {
        id: 'p',
        position: { x: 0, y: 0 },
        inputPorts: [],
        outputPorts: [],
        runtime: { type: 'provider', config },
        appearance: {
          icon: 'VST3',
          title: 'Effect',
          category: 'Processor',
          parameterControls: [1, 2].map((id) => ({
            id: `external:${id}`,
            label: String(id),
            value: id / 10,
            minimum: 0,
            maximum: 1,
            step: 0.01,
            suffix: '',
          })),
        },
      },
    ],
  };
  const document: RuntimeGraphDocument = {
    schemaVersion: 2,
    revision: 1,
    edges: [],
    nodes: [{ id: 'p', type: 'provider', label: null, config }],
  };
  let selected: string | undefined = 'saved';
  const getGraph = vi.fn().mockResolvedValue(document);
  const port = {
    ...graphWriterDefaults(getGraph, () => selected),
    getGraph,
    getGraphStatus: vi
      .fn()
      .mockResolvedValue({ activeRevision: 1, submittedRevision: 1, protocolVersion: 1 }),
    applyGraph: vi.fn(
      async (request: GraphApplyRequest): Promise<GraphApplyResponse> => ({
        protocolVersion: 1,
        revision: request.revision,
        state: 'active',
      }),
    ),
  };
  const callbacks = {
    beforeApply,
    getDocument: () => document,
    getModel: () => draft,
    document: vi.fn(),
    restore: vi.fn(),
    state: vi.fn(),
    error: vi.fn(),
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
    edit: (id: number, value: number) => {
      const before = draft;
      draft = updateRoutingNodeParameter(draft, 'p', `external:${id}`, value);
      return changedProcessorParameters(before, draft);
    },
  };
}
const flush = () => vi.advanceTimersByTimeAsync(0);
function submitEdit(f: ReturnType<typeof fixture>, id: number, value: number) {
  const changes = f.edit(id, value);
  f.queue.submit(f.draft, changes);
}
beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

it('ordinary graph edits never infer parameter intent from stale full snapshots', async () => {
  const f = fixture();
  f.queue.submit(f.draft);
  await flush();
  expect(f.port.applyGraph.mock.calls[0]?.[0]).not.toHaveProperty('processorParameterChanges');
});

it('names the selected graph so another client selecting a graph cannot redirect an edit', async () => {
  const f = fixture();
  f.select('graph-a');
  const queue = new GraphApplicationQueue(
    f.port,
    { ...f.callbacks, getGraphId: () => 'graph-a' },
    workspaceRuntime,
  );
  queue.submit(f.draft);
  await flush();
  expect(f.port.applyGraph.mock.calls[0]?.[0].expectedGraphId).toBe('graph-a');
  // Without a catalog (single-session engines) no graph is named.
  f.queue.submit(f.draft);
  await flush();
  expect(f.port.applyGraph.mock.calls[1]?.[0]).not.toHaveProperty('expectedGraphId');
});

it('derives only explicitly changed controls and supports the inverse Undo/Redo delta', () => {
  const f = fixture();
  const before = f.draft;
  expect(f.edit(1, 0.8)).toEqual([change(1, 0.8)]);
  expect(changedProcessorParameters(f.draft, before)).toEqual([change(1, 0.1)]);
  expect(changedProcessorParameters(before, f.draft)).toEqual([change(1, 0.8)]);
  expect(changedProcessorParameters(f.draft, f.draft)).toEqual([]);
});

it('coalesces explicit IDs without losing other controls behind an in-flight structural edit', async () => {
  const f = fixture();
  const pending = Promise.withResolvers<GraphApplyResponse>();
  f.port.applyGraph.mockReturnValueOnce(pending.promise);
  f.queue.submit(f.draft);
  submitEdit(f, 1, 0.4);
  submitEdit(f, 2, 0.6);
  submitEdit(f, 1, 0.9);
  f.queue.submit(f.draft); // A later structural snapshot must retain still-unacknowledged intent.
  pending.resolve({ protocolVersion: 1, revision: 2, state: 'active' });
  await flush();
  expect(f.port.applyGraph.mock.calls[1]?.[0].processorParameterChanges).toEqual([
    change(1, 0.9),
    change(2, 0.6),
  ]);
  f.queue.submit(f.draft);
  await flush();
  expect(f.port.applyGraph.mock.calls[2]?.[0]).not.toHaveProperty('processorParameterChanges');
});

it('retains failed intent for Retry despite a fresh engine snapshot, with latest same-ID edit winning', async () => {
  const f = fixture();
  const pending = Promise.withResolvers<GraphApplyResponse>();
  f.port.applyGraph.mockReturnValueOnce(pending.promise);
  submitEdit(f, 1, 0.8);
  submitEdit(f, 1, 0.3);
  submitEdit(f, 2, 0.6);
  pending.reject(new Error('offline'));
  await flush();
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
  f.queue.retry();
  await flush();
  expect(f.port.applyGraph.mock.calls[1]?.[0].processorParameterChanges).toEqual([
    change(1, 0.3),
    change(2, 0.6),
  ]);
});

it('Reload and reset discard stale intent instead of applying it to a new document', async () => {
  const f = fixture();
  f.port.applyGraph.mockRejectedValueOnce(new Error('offline'));
  submitEdit(f, 1, 0.8);
  await flush();
  f.queue.reload();
  await flush();
  expect(f.callbacks.restore).toHaveBeenCalledTimes(1);
  f.queue.submit(f.draft);
  await flush();
  expect(f.port.applyGraph.mock.calls[1]?.[0]).not.toHaveProperty('processorParameterChanges');
  f.queue.reset();
});

it('automatic refresh never reads or applies saved reload layout', async () => {
  const f = fixture();
  const getReloadLayout = vi.fn().mockResolvedValue({
    graphId: 'saved',
    layoutRevision: 3,
    layout: { positions: { p: { x: 851, y: 231 } } },
  });
  const callbacks = {
    ...f.callbacks,
    getGraphId: () => 'saved',
    getReloadLayout,
  };
  const queue = new GraphApplicationQueue(f.port, callbacks, workspaceRuntime);
  queue.refresh();
  await flush();
  expect(callbacks.restore).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({ revision: 1 }),
  );
  expect(getReloadLayout).not.toHaveBeenCalled();
});

it('does not restore a late layout reply after graph selection changes', async () => {
  const f = fixture();
  const reply = Promise.withResolvers<{
    graphId: string;
    layoutRevision: number;
    layout: { positions: Record<string, { x: number; y: number }> };
  }>();
  let graphId = 'saved';
  const getReloadLayout = vi.fn().mockReturnValue(reply.promise);
  const callbacks = {
    ...f.callbacks,
    getGraphId: () => graphId,
    getReloadLayout,
  };
  const queue = new GraphApplicationQueue(f.port, callbacks, workspaceRuntime);
  queue.reload();
  await flush();
  expect(getReloadLayout).toHaveBeenCalledExactlyOnceWith('saved');
  graphId = 'other';
  callbacks.error.mockClear();
  callbacks.state.mockClear();
  reply.resolve({ graphId: 'saved', layoutRevision: 3, layout: { positions: {} } });
  await flush();
  expect(callbacks.restore).not.toHaveBeenCalled();
  expect(callbacks.document).not.toHaveBeenCalled();
  expect(callbacks.error).not.toHaveBeenCalled();
  expect(callbacks.state).not.toHaveBeenCalled();
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});

it('does not restore a late layout reply after the queue is reset', async () => {
  const f = fixture();
  const reply = Promise.withResolvers<{
    graphId: string;
    layoutRevision: number;
    layout: { positions: Record<string, { x: number; y: number }> };
  }>();
  const callbacks = {
    ...f.callbacks,
    getGraphId: () => 'saved',
    getReloadLayout: vi.fn().mockReturnValue(reply.promise),
  };
  const queue = new GraphApplicationQueue(f.port, callbacks, workspaceRuntime);
  queue.reload();
  await flush();
  expect(callbacks.getReloadLayout).toHaveBeenCalledOnce();
  queue.reset();
  reply.resolve({ graphId: 'saved', layoutRevision: 3, layout: { positions: {} } });
  await flush();
  expect(callbacks.restore).not.toHaveBeenCalled();
});

it('does not send intent for removed or replaced processor identities', async () => {
  const f = fixture();
  const changes = f.edit(1, 0.8);
  f.queue.submit({ ...f.draft, nodes: [] }, changes);
  await flush();
  expect(f.port.applyGraph.mock.calls[0]?.[0]).not.toHaveProperty('processorParameterChanges');
  const replaced = {
    ...f.draft,
    nodes: f.draft.nodes.map((node) => ({
      ...node,
      runtime: { type: 'provider', config: { typeId: 'other', parameters: {} } },
    })),
  };
  expect(changedProcessorParameters(f.draft, replaced)).toEqual([]);
});

it('waits for already-issued direct parameter IPC before submitting a replacement graph', async () => {
  const direct = Promise.withResolvers<void>();
  const f = fixture(() => direct.promise);
  f.queue.submit(f.draft);
  expect(f.queue.canPublishParameters).toBe(false);
  expect(f.port.applyGraph).not.toHaveBeenCalled();
  direct.resolve();
  await flush();
  expect(f.port.applyGraph).toHaveBeenCalledTimes(1);
});

it('bounds a hung direct parameter barrier and ignores its completion after cancellation', async () => {
  const direct = Promise.withResolvers<void>();
  const f = fixture(() => direct.promise);
  f.queue.submit(f.draft);
  await vi.advanceTimersByTimeAsync(10_001);
  expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
  expect(f.port.applyGraph).not.toHaveBeenCalled();
  f.queue.reset();
  direct.resolve();
  await flush();
  expect(f.port.applyGraph).not.toHaveBeenCalled();
});
