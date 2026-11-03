import { expect, it, vi } from 'vitest';
import { GraphReadback } from '../../core/application/workspace/GraphReadback';
import { fakeScheduler } from '../../test-support/fakeScheduler';

it('waits for a pushed revision on the scheduler it is given', async () => {
  const scheduler = fakeScheduler();
  const port = {
    getGraph: vi.fn(),
    getGraphCatalog: vi.fn(),
    getGraphStatus: vi.fn(() => new Promise<never>(() => undefined)),
    subscribeState: vi.fn(() => () => undefined),
  };
  const readback = new GraphReadback(port, 1000, (code) => new Error(code), scheduler, scheduler);
  const waiting = readback.waitForRevision(2, new AbortController().signal);
  scheduler.advance(1000);
  await expect(waiting).rejects.toThrow('graph_apply_timeout');
});
