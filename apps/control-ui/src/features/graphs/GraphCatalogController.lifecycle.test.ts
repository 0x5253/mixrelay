import { afterEach, expect, it, vi } from 'vitest';
import { monotonicClock, timerScheduler } from '../../core/adapters/browser/timers';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import type { GraphCatalog } from '../../core/domain/control/catalog';
import { ControlPortError } from '../../core/domain/control/errors';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
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
it('serializes initial catalog reads and mutations while a native operation is pending', async () => {
  const { port, controller } = fixture();
  const read = Promise.withResolvers<GraphCatalog>();
  const catalog = await port.getGraphCatalog();
  const get = vi.spyOn(port, 'getGraphCatalog').mockReturnValueOnce(read.promise);
  const pending = controller.reload();
  await controller.reload();
  await controller.mutate({ action: 'create', name: 'Ignored' });
  expect(get).toHaveBeenCalledTimes(1);
  read.resolve(catalog);
  await pending;
  expect(controller.snapshot()).toMatchObject({ busy: false, catalog });
  controller.dispose();
});
it('does not wait for activation when the native workspace file command is unavailable', async () => {
  const { port, controller } = fixture();
  await controller.reload();
  const native: ControlPort = port;
  native.workspaceFile = undefined;
  const status = vi.spyOn(native, 'getGraphStatus');
  await controller.workspaceFile('save', 'en');
  expect(status).not.toHaveBeenCalled();
  controller.dispose();
});
it('rejects a changed graph revision while waiting for native audio activation', async () => {
  const { port, controller } = fixture();
  await controller.reload();
  const next = await port.getGraphCatalog();
  next.document = { ...next.document, revision: 2 };
  offerWorkspaceFile(port, vi.fn().mockResolvedValue(next));
  vi.spyOn(port, 'getGraphStatus').mockResolvedValue({
    protocolVersion: 1,
    submittedRevision: 3,
    activeRevision: 1,
  });
  await controller.workspaceFile('open', 'en');
  expect(controller.snapshot()).toMatchObject({
    document: next.document,
    uncertain: true,
    error: 'control_request_failed',
  });
  controller.dispose();
});
it('times out unconfirmed activation and fences its late status reply', async () => {
  vi.useFakeTimers();
  const { port, controller } = fixture();
  await controller.reload();
  const next = await port.getGraphCatalog();
  next.document = { ...next.document, revision: 2 };
  offerWorkspaceFile(port, vi.fn().mockResolvedValue(next));
  vi.spyOn(port, 'getGraphStatus').mockResolvedValue({
    protocolVersion: 1,
    submittedRevision: 2,
    activeRevision: 1,
  });
  const pending = controller.workspaceFile('open', 'en');
  await vi.advanceTimersByTimeAsync(10_100);
  await pending;
  expect(controller.snapshot()).toMatchObject({ uncertain: true, error: 'control_request_failed' });
  controller.dispose();
});
it('does not send a catalog mutation after disposal during document revision lookup', async () => {
  const { port, controller } = fixture();
  await controller.reload();
  const document = Promise.withResolvers<RuntimeGraphDocument>();
  vi.spyOn(port, 'getGraph').mockReturnValueOnce(document.promise);
  const write = vi.spyOn(port, 'mutateGraphCatalog');
  const pending = controller.mutate({ action: 'create', name: 'Retired' });
  controller.dispose();
  document.resolve({ schemaVersion: 2, revision: 1, nodes: [], edges: [] });
  await pending;
  expect(write).not.toHaveBeenCalled();
});
it('treats a server failure with unrelated structured parameters as an uncertain file outcome', async () => {
  const { port, controller } = fixture();
  await controller.reload();
  offerWorkspaceFile(
    port,
    vi.fn().mockRejectedValue(
      new ControlPortError(500, {
        code: 'control_unavailable',
        parameters: { reason: 'IPC' },
      }),
    ),
  );
  await controller.workspaceFile('open', 'en');
  expect(controller.snapshot().uncertain).toBe(true);
  const status = vi.spyOn(port, 'getGraphStatus');
  await controller.workspaceFile('save', 'en');
  expect(status).not.toHaveBeenCalled();
  controller.dispose();
});

it('clears a confirmed rejected-file error without introducing an uncertain mutation outcome', async () => {
  const { port, controller } = fixture();
  await controller.reload();
  offerWorkspaceFile(
    port,
    vi.fn().mockRejectedValue(
      new ControlPortError(503, {
        code: 'control_unavailable',
        parameters: { workspaceOutcome: 'not_applied' },
      }),
    ),
  );
  await controller.workspaceFile('open', 'en');
  expect(controller.snapshot()).toMatchObject({ uncertain: false, error: 'control_unavailable' });
  controller.clearError();
  expect(controller.snapshot().error).toBeNull();
  controller.dispose();
});
it('fences file confirmation if a subscriber replaces the owner as activation begins', async () => {
  const { port, controller } = fixture();
  await controller.reload();
  const next = await port.getGraphCatalog();
  offerWorkspaceFile(port, vi.fn().mockResolvedValue(next));
  const status = vi.spyOn(port, 'getGraphStatus');
  const unsubscribe = controller.subscribe(() => {
    if (!controller.snapshot().uncertain) return;
    unsubscribe();
    controller.dispose();
  });
  await controller.workspaceFile('open', 'en');
  expect(status).toHaveBeenCalledTimes(1);
  expect(controller.snapshot().busy).toBe(false);
});
