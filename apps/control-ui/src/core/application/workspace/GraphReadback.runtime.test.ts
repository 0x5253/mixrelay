import { expect, it, vi } from 'vitest';
import { fakeScheduler } from '../../../test-support/fakeScheduler';
import type { GraphRuntimeStatus } from '../../domain/control/graph';
import { GraphReadback, pauseGraphObservation } from './GraphReadback';

const flush = async () => {
  for (let turn = 0; turn < 10; turn += 1) await Promise.resolve();
};

it('bounds each polling read with its injected elapsed clock and cancels pending work', async () => {
  const time = fakeScheduler();
  const status: GraphRuntimeStatus = {
    protocolVersion: 1,
    submittedRevision: 2,
    activeRevision: 1,
  };
  const port = {
    getGraph: vi.fn(),
    getGraphCatalog: vi.fn(),
    getGraphStatus: vi
      .fn()
      .mockResolvedValueOnce(status)
      .mockReturnValue(new Promise(() => {})),
  };
  const readback = new GraphReadback(port, 1000, (code) => new Error(code), time, time);
  const outcome = readback.waitForRevision(2, new AbortController().signal).catch((error) => error);
  await flush();
  time.advance(200);
  await flush();
  expect(port.getGraphStatus).toHaveBeenCalledTimes(2);
  time.advance(799);
  expect(time.pending()).toBe(1);
  time.advance(1);
  expect(await outcome).toEqual(new Error('graph_apply_timeout'));
  expect(time.pending()).toBe(0);
});

it('rejects an already retired observation without scheduling a pause', async () => {
  const time = fakeScheduler();
  const retired = AbortSignal.abort(new Error('retired selection'));
  await expect(pauseGraphObservation(retired, time)).rejects.toThrow('retired selection');
  expect(time.pending()).toBe(0);
});
