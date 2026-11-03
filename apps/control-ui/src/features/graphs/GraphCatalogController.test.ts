import { afterEach, describe, expect, it, vi } from 'vitest';
import { monotonicClock, timerScheduler } from '../../core/adapters/browser/timers';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import type { GraphCatalog } from '../../core/domain/control/catalog';
import { ControlPortError } from '../../core/domain/control/errors';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import type { CatalogPort } from './application/GraphCatalogController';
import { GraphCatalogController } from './application/GraphCatalogController';

/** Gives the fake engine the optional workspaceFile capability. */
function offerWorkspaceFile(port: CatalogPort, value: CatalogPort['workspaceFile']) {
  port.workspaceFile = value;
}

function fixture() {
  const port = new StorybookMockControlPort({
    graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
  });
  return { port, controller: new GraphCatalogController(port, timerScheduler, monotonicClock) };
}

afterEach(() => vi.useRealTimers());

describe('workspace files', () => {
  it('does not send a file mutation after its owner is disposed during revision lookup', async () => {
    const { port, controller } = fixture();
    await controller.reload();
    const status = Promise.withResolvers<Awaited<ReturnType<ControlPort['getGraphStatus']>>>();
    vi.spyOn(port, 'getGraphStatus').mockReturnValueOnce(status.promise);
    const file = vi.fn().mockResolvedValue(null);
    offerWorkspaceFile(port, file);
    const pending = controller.workspaceFile('new', 'en');
    controller.dispose();
    await controller.reload();
    const current = controller.snapshot();
    status.resolve({ protocolVersion: 1, submittedRevision: 1, activeRevision: 1 });
    await pending;
    expect(file).not.toHaveBeenCalled();
    expect(controller.snapshot()).toBe(current);
  });

  it.each(['accept', 'reject'] as const)(
    'ignores a late file %s after a replacement catalog load',
    async (outcome) => {
      const { port, controller } = fixture();
      await controller.reload();
      const result = Promise.withResolvers<GraphCatalog | null>();
      const file = vi.fn().mockReturnValue(result.promise);
      offerWorkspaceFile(port, file);
      const pending = controller.workspaceFile('open', 'en');
      await vi.waitFor(() => expect(file).toHaveBeenCalledTimes(1));
      controller.dispose();
      await controller.reload();
      const current = controller.snapshot();
      const read = vi.spyOn(port, 'getGraphCatalog');
      if (outcome === 'accept') result.resolve(current.catalog);
      else result.reject(new Error('old file operation failed'));
      await pending;
      expect(controller.snapshot()).toBe(current);
      expect(read).not.toHaveBeenCalled();
    },
  );

  it('stops a file continuation when its accepted-catalog subscriber replaces the owner', async () => {
    const { port, controller } = fixture();
    await controller.reload();
    const loaded = await port.getGraphCatalog();
    offerWorkspaceFile(port, vi.fn().mockResolvedValue(loaded));
    const replacement = Promise.withResolvers<void>();
    const replacementStates: boolean[] = [];
    const unsubscribe = controller.subscribe(() => {
      if (controller.snapshot().catalog !== loaded) return;
      unsubscribe();
      controller.subscribe(() => replacementStates.push(controller.snapshot().uncertain));
      controller.dispose();
      void controller.reload().then(replacement.resolve);
    });
    await controller.workspaceFile('open', 'en');
    await replacement.promise;
    // Include intermediate notifications, not just the final read response.
    expect(replacementStates).not.toContain(true);
    expect(controller.snapshot().uncertain).toBe(false);
  });
});

describe('workspace file outcomes', () => {
  it('cancel and save preserve document identity and history', async () => {
    const { port, controller } = fixture();
    await controller.reload();
    const before = controller.snapshot();
    const file = vi.fn().mockResolvedValueOnce(null).mockResolvedValueOnce(before.catalog);
    offerWorkspaceFile(port, file);
    await controller.workspaceFile('open', 'ko');
    await controller.workspaceFile('save', 'ko');
    expect(controller.snapshot().document).toBe(before.document);
    expect(controller.snapshot().documentGeneration).toBe(before.documentGeneration);
    expect(controller.snapshot().uncertain).toBe(false);
  });

  it('retains the accepted file when subsequent capture synchronization fails', async () => {
    const { port, controller } = fixture();
    await controller.reload();
    const before = controller.snapshot();
    const loaded = await port.getGraphCatalog();
    loaded.document = { ...loaded.document, nodes: [], revision: 2 };
    offerWorkspaceFile(port, vi.fn().mockResolvedValue(loaded));
    vi.spyOn(port, 'getGraph').mockRejectedValue(new Error('old capture unavailable'));
    vi.spyOn(port, 'getGraphStatus').mockResolvedValue({
      protocolVersion: 1,
      activeRevision: 2,
      submittedRevision: 2,
    });
    vi.spyOn(port, 'getGraphCatalog').mockRejectedValue(new Error('capture permission'));
    await controller.workspaceFile('open', 'ko');
    expect(controller.snapshot().document).toBe(loaded.document);
    expect(controller.snapshot().documentGeneration).toBe(before.documentGeneration + 1);
    expect(controller.snapshot().uncertain).toBe(true);
  });

  it('fences ambiguous server failures but not rejected invalid files', async () => {
    for (const status of [422, 503]) {
      const { port, controller } = fixture();
      await controller.reload();
      offerWorkspaceFile(
        port,
        vi
          .fn()
          .mockRejectedValue(
            new ControlPortError(status, { code: 'graph_invalid', parameters: {} }),
          ),
      );
      await controller.workspaceFile('open', 'en');
      expect(controller.snapshot().uncertain).toBe(status === 503);
    }
  });
});

describe('durable graph catalog coordinator', () => {
  it('deletes the last graph with a fresh empty identity and fences late retries', async () => {
    const { controller } = fixture();
    await controller.reload();
    const first = controller.snapshot().catalog?.activeId ?? '';
    await controller.mutate({ action: 'delete', id: first });
    const after = controller.snapshot();
    expect(after.catalog?.activeId).not.toBe(first);
    expect(after.catalog?.graphs).toHaveLength(1);
    expect(after.document?.nodes).toEqual([]);
    await controller.mutate({ action: 'delete', id: first });
    expect(controller.snapshot().catalog).toBe(after.catalog);
  });

  it('blocks further destructive edits after an ambiguous server failure', async () => {
    const { port, controller } = fixture();
    await controller.reload();
    const mutate = vi
      .spyOn(port, 'mutateGraphCatalog')
      .mockRejectedValue(
        new ControlPortError(503, { code: 'control_unavailable', parameters: {} }),
      );
    await controller.mutate({ action: 'delete', id: 'first' });
    expect(controller.snapshot().uncertain).toBe(true);
    await controller.mutate({ action: 'delete', id: 'second' });
    expect(mutate).toHaveBeenCalledTimes(1);
  });
  it('creates independent graphs and confirms activation before replacing the workspace', async () => {
    const { port, controller } = fixture();
    await controller.reload();
    const original = controller.snapshot().document;
    const mutate = vi.spyOn(port, 'mutateGraphCatalog');
    await controller.mutate({ action: 'create', name: 'Broadcast' });
    expect(mutate).toHaveBeenCalledWith({
      action: 'create',
      name: 'Broadcast',
      expectedRevision: 1,
      expectedCatalogRevision: 1,
    });
    expect(controller.snapshot().catalog?.graphs).toHaveLength(2);
    expect(controller.snapshot().document).not.toBe(original);
    expect(controller.snapshot().catalog?.activeId).toBe('graph-2');
    await controller.mutate({ action: 'select', id: 'graph-1' });
    expect(controller.snapshot().catalog?.activeId).toBe('graph-1');
  });

  it('does not reset document/history on metadata-only rename', async () => {
    const { controller } = fixture();
    await controller.reload();
    const document = controller.snapshot().document;
    const generation = controller.snapshot().documentGeneration;
    await controller.mutate({ action: 'rename', id: 'graph-1', name: 'Ableton' });
    expect(controller.snapshot().document).toBe(document);
    expect(controller.snapshot().documentGeneration).toBe(generation);
    expect(controller.snapshot().catalog?.graphs[0]?.name).toBe('Ableton');
  });

  it('retains selection/document after native rejection and preserves retryable error', async () => {
    const { port, controller } = fixture();
    await controller.reload();
    const before = controller.snapshot().catalog;
    vi.spyOn(port, 'mutateGraphCatalog').mockRejectedValue(
      new ControlPortError(422, { code: 'graph_invalid', parameters: {} }),
    );
    await controller.mutate({ action: 'create', name: 'Rejected' });
    expect(controller.snapshot().catalog).toBe(before);
    expect(controller.snapshot()).toMatchObject({
      error: 'graph_invalid',
      uncertain: false,
      busy: false,
    });
  });

  it('blocks edits after a lost mutation acknowledgement until authoritative reload', async () => {
    const { port, controller } = fixture();
    await controller.reload();
    const mutate = vi.spyOn(port, 'mutateGraphCatalog').mockRejectedValue(new Error('lost reply'));
    await controller.mutate({ action: 'create', name: 'Maybe applied' });
    expect(controller.snapshot().uncertain).toBe(true);
    await controller.mutate({ action: 'create', name: 'Must not send' });
    expect(mutate).toHaveBeenCalledTimes(1);
    await controller.reload();
    expect(controller.snapshot().uncertain).toBe(false);
  });
});

describe('catalog read recovery', () => {
  it('backs off repeated failures and caps polling at five seconds', async () => {
    vi.useFakeTimers();
    const { port, controller } = fixture();
    const read = vi.spyOn(port, 'getGraphCatalog').mockRejectedValue(new Error('offline'));
    await controller.reload();
    for (const [index, delay] of [500, 1000, 2000, 4000, 5000, 5000].entries()) {
      await vi.advanceTimersByTimeAsync(delay - 1);
      expect(read).toHaveBeenCalledTimes(index + 1);
      await vi.advanceTimersByTimeAsync(1);
      expect(read).toHaveBeenCalledTimes(index + 2);
    }
    controller.dispose();
  });

  it.each([
    {},
    { resource: 'application_capture_manifest', reason: 'graph_mutation_in_progress' },
    { resource: 'application_capture_manifest', reason: 'execution_snapshot_not_published' },
  ])(
    'recovers the initial catalog automatically after temporary unavailability: %j',
    async (parameters) => {
      vi.useFakeTimers();
      const { port, controller } = fixture();
      const read = vi
        .spyOn(port, 'getGraphCatalog')
        .mockRejectedValueOnce(
          new ControlPortError(503, { code: 'control_unavailable', parameters }),
        );
      await controller.reload();
      expect(controller.snapshot().catalog).toBeNull();
      await vi.advanceTimersByTimeAsync(500);
      expect(read).toHaveBeenCalledTimes(2);
      expect(controller.snapshot().catalog?.activeId).toBe('graph-1');
      expect(controller.snapshot().error).toBeNull();
      controller.dispose();
    },
  );

  it('does not retry a protected catalog or replay failed mutations', async () => {
    vi.useFakeTimers();
    const { port, controller } = fixture();
    const read = vi.spyOn(port, 'getGraphCatalog').mockRejectedValue(
      new ControlPortError(503, {
        code: 'control_unavailable',
        parameters: { resource: 'graph_catalog', reason: 'invalid file' },
      }),
    );
    await controller.reload();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(read).toHaveBeenCalledTimes(1);
    controller.dispose();
  });

  it('cancels scheduled recovery on disposal and never overwrites a loaded workspace', async () => {
    vi.useFakeTimers();
    const { port, controller } = fixture();
    const read = vi.spyOn(port, 'getGraphCatalog').mockRejectedValueOnce(new Error('offline'));
    await controller.reload();
    controller.dispose();
    await vi.advanceTimersByTimeAsync(5000);
    expect(read).toHaveBeenCalledTimes(1);
    await controller.reload();
    const document = controller.snapshot().document;
    read.mockRejectedValue(new Error('offline'));
    await controller.reload();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(read).toHaveBeenCalledTimes(3);
    expect(controller.snapshot().document).toBe(document);
    controller.dispose();
  });
});

describe('catalog operation lifetime', () => {
  it('distinguishes saved-graph storage failure from an engine connection failure', async () => {
    const { port, controller } = fixture();
    await controller.reload();
    vi.spyOn(port, 'mutateGraphCatalog').mockRejectedValue(
      new ControlPortError(503, {
        code: 'control_unavailable',
        parameters: {
          resource: 'graph_catalog',
          reason: 'disk unavailable',
          catalogOutcome: 'not_applied',
        },
      }),
    );
    await controller.mutate({ action: 'create', name: 'Unsaved' });
    expect(controller.snapshot()).toMatchObject({
      error: 'graph_catalog_unavailable',
      uncertain: false,
    });
    expect(controller.snapshot().catalog?.activeId).toBe('graph-1');
  });
  it('bounds a stalled request and fences late responses after dispose', async () => {
    vi.useFakeTimers();
    const { port, controller } = fixture();
    let resolve: (catalog: GraphCatalog) => void = () => {};
    const catalog = await port.getGraphCatalog();
    vi.spyOn(port, 'getGraphCatalog').mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const pending = controller.reload();
    await vi.advanceTimersByTimeAsync(10_000);
    await pending;
    expect(controller.snapshot()).toMatchObject({ busy: false, error: 'control_request_failed' });
    controller.dispose();
    resolve(catalog);
    await Promise.resolve();
    expect(controller.snapshot().catalog).toBeNull();
  });

  it('waits for active revision and ignores late activation after disposal', async () => {
    vi.useFakeTimers();
    const { port, controller } = fixture();
    await controller.reload();
    const status = vi
      .spyOn(port, 'getGraphStatus')
      .mockResolvedValue({ protocolVersion: 1, submittedRevision: 2, activeRevision: 1 });
    const pending = controller.mutate({ action: 'create', name: 'pending' });
    await vi.advanceTimersByTimeAsync(1);
    expect(controller.snapshot().catalog?.activeId).toBe('graph-1');
    expect(controller.snapshot().busy).toBe(true);
    status.mockResolvedValue({ protocolVersion: 1, submittedRevision: 2, activeRevision: 2 });
    controller.dispose();
    await vi.advanceTimersByTimeAsync(100);
    await pending;
    expect(controller.snapshot().catalog?.activeId).toBe('graph-1');
  });
});
