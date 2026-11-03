import { expect, it, vi } from 'vitest';
import type { MeterSnapshot } from '../../../core/domain/control/meters';
import { fakeScheduler } from '../../../test-support/fakeScheduler';
import { MeterRequestOwner, MeterSession } from './MeterSession';

it('reuses a pending native read across visibility replay and rejects its expired sample', async () => {
  const time = fakeScheduler();
  const response = Promise.withResolvers<MeterSnapshot>();
  const port = { getMeters: vi.fn(() => response.promise) };
  const output = { update: vi.fn(), clear: vi.fn() };
  const requests = new MeterRequestOwner(port, time);
  const first = new MeterSession(port, requests, output, time, time);
  first.setVisible(true);
  await Promise.resolve();
  first.dispose();
  time.advance(300);
  const second = new MeterSession(port, requests, output, time, time);
  second.setVisible(true);
  response.resolve({ protocolVersion: 1, revision: 1, nodes: [] });
  await response.promise;
  await Promise.resolve();
  expect(port.getMeters).toHaveBeenCalledTimes(1);
  expect(output.update).not.toHaveBeenCalled();
  expect(output.clear).toHaveBeenCalled();
  second.dispose();
  expect(time.pending()).toBe(0);
});
