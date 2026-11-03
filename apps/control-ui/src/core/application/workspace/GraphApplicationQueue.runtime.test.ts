import { expect, it, vi } from 'vitest';
import { fakeScheduler } from '../../../test-support/fakeScheduler';
import { graphWriterDefaults } from '../../../test-support/portDefaults';
import type { GraphApplyRequest, RuntimeGraphDocument } from '../../domain/control/graph';
import { GraphApplicationQueue } from './GraphApplicationQueue';

const document: RuntimeGraphDocument = {
  schemaVersion: 2,
  revision: 3,
  edges: [],
  nodes: [{ id: 'gain', type: 'gain', label: 'Original', config: {} }],
};

const flush = async () => {
  for (let turn = 0; turn < 30; turn += 1) await Promise.resolve();
};

function fixture() {
  const time = fakeScheduler();
  const draft = { label: 'Edited' };
  const getGraph = vi.fn(async () => document);
  const applyGraph = vi.fn(async (_request: GraphApplyRequest) => ({
    protocolVersion: 1 as const,
    revision: 4,
    state: 'active' as const,
  }));
  const port = {
    ...graphWriterDefaults(getGraph, () => 'a'),
    getGraph,
    applyGraph,
    getGraphStatus: vi.fn(async () => ({
      protocolVersion: 1 as const,
      activeRevision: 3,
      submittedRevision: 3,
    })),
  };
  const view = {
    getGraphId: () => 'a',
    getDocument: () => document,
    getModel: () => draft,
    document: vi.fn(),
    restore: vi.fn(),
    state: vi.fn(),
    error: vi.fn(),
  };
  const queue = new GraphApplicationQueue(port, view, {
    scheduler: time,
    clock: time,
    operationId: () => 'operation-from-owner',
    toDocument: (model, source) => ({
      ...source,
      nodes: source.nodes.map((node) => ({ ...node, label: model.label })),
    }),
  });
  return { queue, view, port, draft, time };
}

it('projects an arbitrary draft through the owner port and publishes only the acknowledged revision', async () => {
  const f = fixture();
  f.queue.submit(f.draft);
  expect(f.view.document).not.toHaveBeenCalled();
  await flush();
  expect(f.port.applyGraph).toHaveBeenCalledWith({
    ...document,
    revision: 4,
    nodes: [{ ...document.nodes[0], label: 'Edited' }],
    expectedGraphId: 'a',
    clientOperationId: 'operation-from-owner',
  });
  expect(f.view.document).toHaveBeenCalledWith(
    expect.objectContaining({ revision: 4, nodes: [expect.objectContaining({ label: 'Edited' })] }),
  );
  expect(f.view.state).toHaveBeenLastCalledWith('active');
  expect(f.time.pending()).toBe(0);
});

it('keeps an uncertain command blocked without replaying it when injected time advances', async () => {
  const f = fixture();
  f.port.applyGraph.mockRejectedValue(new Error('acknowledgement lost'));
  f.queue.submit(f.draft);
  await flush();
  expect(f.view.state).toHaveBeenLastCalledWith('blocked');
  expect(f.view.restore).not.toHaveBeenCalled();
  expect(f.queue.canPublishParameters).toBe(false);
  f.time.advance(60_000);
  await flush();
  expect(f.port.applyGraph).toHaveBeenCalledOnce();
  expect(f.time.pending()).toBe(0);
});
