import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  GRAPH_APPLICATION_TIMEOUT_MS,
  GraphApplicationQueue,
} from '../../core/application/workspace/GraphApplicationQueue';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import { ControlPortError } from '../../core/domain/control/errors';
import type {
  GraphApplyResponse,
  ProcessorRetryRequest,
  RuntimeGraphDocument,
} from '../../core/domain/control/graph';
import { graphWriterDefaults } from '../../test-support/portDefaults';
import { workspaceRuntime } from '../../test-support/workspaceRuntime';
import { runtimeGraphToAudioGraph } from './runtimeGraphProjection';

const document: RuntimeGraphDocument = {
  edges: [],
  nodes: [
    {
      config: {
        typeId: 'vendor.plugin',
        ports: {
          layoutId: 'layout-v1:stereo',
          inputs: [
            {
              id: 'audio',
              name: 'Main Input',
              kind: 'main',
              channels: 2,
              channelFormat: 'stereo',
            },
          ],
          outputs: [
            {
              id: 'audio',
              name: 'Main Output',
              kind: 'main',
              channels: 2,
              channelFormat: 'stereo',
            },
          ],
        },
      },
      control: { enabled: true, muted: false },
      id: 'provider-a',
      label: 'Crashy',
      type: 'provider',
    },
  ],
  revision: 7,
  schemaVersion: 2,
};

const target = {
  expectedCatalogRevision: 12,
  expectedInstanceId: '18446744073709551615',
  graphId: 'graph-a',
  nodeId: 'provider-a',
};

const processorNode = document.nodes[0];
if (!processorNode) throw new Error('fixture processor is missing');
const disabledDocument: RuntimeGraphDocument = {
  ...document,
  nodes: [{ ...processorNode, control: { enabled: false, muted: false } }],
};
const deletedDocument: RuntimeGraphDocument = { ...document, nodes: [] };

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((onResolve, onReject) => {
    resolve = onResolve;
    reject = onReject;
  });
  return { promise, reject, resolve };
}

function fixture(options: { beforeApply?: () => Promise<void> } = {}) {
  let graphId = 'graph-a';
  let catalogRevision = 12;
  let currentDocument = document;
  let currentModel: AudioGraphModel = runtimeGraphToAudioGraph(document);
  let failedInstanceId: string | undefined = target.expectedInstanceId;
  const getGraph = vi.fn(async () => currentDocument);
  const port = {
    ...graphWriterDefaults(getGraph, () => graphId),
    applyGraph: vi.fn(
      async (next: RuntimeGraphDocument): Promise<GraphApplyResponse> => ({
        protocolVersion: 1,
        revision: next.revision,
        state: 'active',
      }),
    ),
    getGraph,
    getGraphStatus: vi.fn(async () => ({
      activeRevision: currentDocument.revision,
      protocolVersion: 1 as const,
      submittedRevision: currentDocument.revision,
    })),
    retryProcessor: vi.fn<(request: ProcessorRetryRequest) => Promise<GraphApplyResponse>>(
      async () => ({
        protocolVersion: 1 as const,
        revision: document.revision + 1,
        state: 'active',
      }),
    ),
  };
  const callbacks = {
    beforeApply: options.beforeApply,
    getGraphId: () => graphId,
    getCatalogRevision: () => catalogRevision,
    hasFailedProcessor: (_graphId: string, _nodeId: string, instanceId: string) =>
      failedInstanceId === instanceId,
    getDocument: () => currentDocument,
    getModel: () => currentModel,
    document: vi.fn((next: RuntimeGraphDocument) => {
      currentDocument = next;
    }),
    restore: vi.fn(),
    state: vi.fn(),
    error: vi.fn(),
    persistenceWarning: vi.fn(),
  };
  return {
    callbacks,
    port,
    queue: new GraphApplicationQueue(port, callbacks, workspaceRuntime),
    get currentDocument() {
      return currentDocument;
    },
    get currentModel() {
      return currentModel;
    },
    setCatalogRevision: (next: number) => {
      catalogRevision = next;
    },
    setDocument: (next: RuntimeGraphDocument) => {
      currentDocument = next;
    },
    setFailedInstance: (next: string | undefined) => {
      failedInstanceId = next;
    },
    setGraphId: (next: string) => {
      graphId = next;
    },
    setModel: (next: AudioGraphModel) => {
      currentModel = next;
    },
  };
}

afterEach(() => vi.useRealTimers());

describe('GraphApplicationQueue crashed processor retry', () => {
  it('serializes duplicate clicks and advances only the acknowledged revision', async () => {
    const response = deferred<GraphApplyResponse>();
    const f = fixture();
    f.port.retryProcessor.mockReturnValueOnce(response.promise);
    const first = f.queue.retryFailedProcessor(target);

    await expect(f.queue.retryFailedProcessor(target)).resolves.toEqual({
      state: 'rejected',
      code: 'graph_exchange_busy',
    });
    expect(f.port.retryProcessor).toHaveBeenCalledExactlyOnceWith({
      clientOperationId: expect.any(String),
      ...target,
      expectedRevision: document.revision,
    });

    response.resolve({ protocolVersion: 1, revision: 8, state: 'active' });
    await expect(first).resolves.toEqual({ state: 'applied', revision: 8 });
    expect(f.callbacks.document).toHaveBeenCalledExactlyOnceWith({ ...document, revision: 8 });
    expect(f.callbacks.restore).not.toHaveBeenCalled();
    expect(f.port.applyGraph).not.toHaveBeenCalled();
    expect(f.port.retryProcessor).toHaveBeenCalledTimes(1);
  });

  it('waits for a pending retry revision to become active', async () => {
    vi.useFakeTimers();
    const f = fixture();
    f.port.retryProcessor.mockResolvedValueOnce({
      protocolVersion: 1,
      revision: 8,
      state: 'pending',
    });
    f.port.getGraphStatus
      .mockResolvedValueOnce({ activeRevision: 7, submittedRevision: 8, protocolVersion: 1 })
      .mockResolvedValueOnce({ activeRevision: 8, submittedRevision: 8, protocolVersion: 1 });

    const retry = f.queue.retryFailedProcessor(target);
    await vi.advanceTimersByTimeAsync(200);
    await expect(retry).resolves.toEqual({ state: 'applied', revision: 8 });
    expect(f.port.retryProcessor).toHaveBeenCalledTimes(1);
    expect(f.callbacks.document).toHaveBeenCalledExactlyOnceWith({ ...document, revision: 8 });
    expect(vi.getTimerCount()).toBe(0);
  });

  it('acknowledges the accepted revision when polling observes it before the response settles', async () => {
    vi.useFakeTimers();
    const f = fixture();
    const draft = { ...f.currentModel, revision: f.currentModel.revision + 1 };
    f.port.retryProcessor.mockResolvedValueOnce({
      protocolVersion: 1,
      revision: 8,
      state: 'pending',
    });
    f.port.getGraphStatus
      .mockImplementationOnce(async () => {
        f.setCatalogRevision(13);
        f.setDocument({ ...document, revision: 8 });
        f.setModel(draft);
        return { activeRevision: 7, submittedRevision: 8, protocolVersion: 1 };
      })
      .mockResolvedValueOnce({ activeRevision: 8, submittedRevision: 8, protocolVersion: 1 });

    const retry = f.queue.retryFailedProcessor(target);
    await vi.advanceTimersByTimeAsync(200);

    await expect(retry).resolves.toEqual({ state: 'applied', revision: 8 });
    expect(f.callbacks.document).toHaveBeenCalledExactlyOnceWith({ ...document, revision: 8 });
    expect(f.callbacks.restore).not.toHaveBeenCalled();
    expect(f.currentModel).toBe(draft);
    expect(f.port.retryProcessor).toHaveBeenCalledTimes(1);
    expect(f.port.applyGraph).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe('GraphApplicationQueue crashed processor retry guards', () => {
  it('does not send when graph or catalog revision changes while waiting to apply', async () => {
    const beforeApply = deferred<void>();
    const f = fixture({ beforeApply: () => beforeApply.promise });
    const retry = f.queue.retryFailedProcessor(target);
    f.setCatalogRevision(13);
    beforeApply.resolve();

    await expect(retry).resolves.toEqual({ state: 'rejected', code: 'graph_revision_stale' });
    expect(f.port.retryProcessor).not.toHaveBeenCalled();

    const secondBeforeApply = deferred<void>();
    const second = fixture({ beforeApply: () => secondBeforeApply.promise });
    const secondRetry = second.queue.retryFailedProcessor(target);
    second.setDocument({ ...document, revision: 9 });
    secondBeforeApply.resolve();

    await expect(secondRetry).resolves.toEqual({
      state: 'rejected',
      code: 'graph_revision_stale',
    });
    expect(second.port.retryProcessor).not.toHaveBeenCalled();
  });

  it.each([
    { code: 'node_disabled', document: disabledDocument, reason: 'off' },
    { code: 'node_not_found', document: deletedDocument, reason: 'deleted' },
  ])('does not retry a $reason processor', async ({ document: nextDocument, code }) => {
    const f = fixture();
    f.setDocument(nextDocument);

    await expect(f.queue.retryFailedProcessor(target)).resolves.toEqual({
      state: 'rejected',
      code,
    });
    expect(f.port.retryProcessor).not.toHaveBeenCalled();
  });
});

describe('GraphApplicationQueue crashed processor retry outcomes', () => {
  it('does not publish an old graph reply after selection changes', async () => {
    const response = deferred<GraphApplyResponse>();
    const f = fixture();
    f.port.retryProcessor.mockReturnValueOnce(response.promise);
    const retry = f.queue.retryFailedProcessor(target);
    f.setGraphId('graph-b');
    response.resolve({ protocolVersion: 1, revision: 8, state: 'active' });

    await expect(retry).resolves.toEqual({
      state: 'uncertain',
      code: 'graph_revision_stale',
    });
    expect(f.callbacks.document).not.toHaveBeenCalled();
    expect(f.callbacks.state).toHaveBeenCalledTimes(1);
    expect(f.callbacks.state).toHaveBeenCalledWith('applying');
    expect(f.callbacks.restore).not.toHaveBeenCalled();
  });

  it('keeps edits made in flight and never resubmits after an uncertain retry', async () => {
    const response = deferred<GraphApplyResponse>();
    const f = fixture();
    f.port.retryProcessor.mockReturnValueOnce(response.promise);
    const retry = f.queue.retryFailedProcessor(target);
    const draft = { ...f.currentModel, revision: f.currentModel.revision + 1 };
    f.setModel(draft);
    f.queue.submit(draft);
    f.port.getGraph.mockResolvedValueOnce({ ...document, revision: 8 });
    response.resolve({ protocolVersion: 1, revision: 8, state: 'active' });

    await expect(retry).resolves.toEqual({ state: 'uncertain', code: 'graph_draft_changed' });
    expect(f.port.retryProcessor).toHaveBeenCalledTimes(1);
    expect(f.port.applyGraph).not.toHaveBeenCalled();
    expect(f.callbacks.restore).not.toHaveBeenCalled();
    expect(f.currentModel).toBe(draft);
    expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
  });

  it('does not automatically replay a request rejected by the engine', async () => {
    const f = fixture();
    f.port.retryProcessor.mockRejectedValueOnce(
      new ControlPortError(503, { code: 'processor_unavailable', parameters: {} }),
    );

    await expect(f.queue.retryFailedProcessor(target)).resolves.toEqual({
      state: 'rejected',
      code: 'processor_unavailable',
    });
    expect(f.port.retryProcessor).toHaveBeenCalledTimes(1);
    expect(f.port.applyGraph).not.toHaveBeenCalled();
  });

  it('bounds stalled IPC and leaves the retry uncertain without a second command', async () => {
    vi.useFakeTimers();
    const f = fixture();
    f.port.retryProcessor.mockImplementationOnce(() => new Promise(() => {}));

    const retry = f.queue.retryFailedProcessor(target);
    await vi.advanceTimersByTimeAsync(GRAPH_APPLICATION_TIMEOUT_MS + 1);
    await expect(retry).resolves.toEqual({ state: 'uncertain', code: 'graph_apply_timeout' });
    expect(f.port.retryProcessor).toHaveBeenCalledTimes(1);
    expect(f.port.applyGraph).not.toHaveBeenCalled();
    expect(f.callbacks.state).toHaveBeenLastCalledWith('blocked');
    expect(vi.getTimerCount()).toBe(0);
  });
});
