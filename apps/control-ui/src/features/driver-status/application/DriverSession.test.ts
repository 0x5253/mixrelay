import { expect, it, vi } from 'vitest';
import type { DriverSnapshot } from '../../../core/domain/driver';
import { fakeScheduler } from '../../../test-support/fakeScheduler';
import { readyDriver } from '../driverFixtures';
import { DriverSession } from './DriverSession';

const flush = async () => {
  for (let turn = 0; turn < 10; turn += 1) await Promise.resolve();
};

it('restarts reads while an old mutation is pending and never republishes or replays its late reply', async () => {
  const time = fakeScheduler();
  const pending = Promise.withResolvers<DriverSnapshot>();
  const current = { ...readyDriver, revision: 8 };
  const port = {
    getSnapshot: vi.fn().mockResolvedValueOnce(readyDriver).mockResolvedValue(current),
    mutate: vi.fn(() => pending.promise),
  };
  const session = new DriverSession(port, time);
  session.start();
  await flush();
  const operation = session.mutate({
    action: 'set_sample_format',
    endpointId: 'device-a',
    sampleFormat: 'float32',
  });
  expect(session.snapshot().busy).toBe(true);
  session.dispose();
  session.start();
  await flush();
  expect(session.snapshot()).toMatchObject({ snapshot: current, busy: false, error: null });
  expect(port.getSnapshot).toHaveBeenCalledTimes(2);
  pending.resolve({ ...readyDriver, revision: 99 });
  expect(await operation).toBeNull();
  expect(session.snapshot().snapshot).toBe(current);
  expect(port.mutate).toHaveBeenCalledOnce();
  session.dispose();
  expect(time.pending()).toBe(0);
});

it.each(['read', 'write'] as const)(
  'disposal during busy publication prevents a subsequent Driver %s',
  async (action) => {
    const time = fakeScheduler();
    const port = { getSnapshot: vi.fn().mockResolvedValue(readyDriver), mutate: vi.fn() };
    const owner = new DriverSession(port, time);
    owner.start();
    await flush();
    const unsubscribe = owner.subscribe(() => {
      if (owner.snapshot().busy) owner.dispose();
    });
    if (action === 'read') await owner.refresh();
    else
      await owner.mutate({
        action: 'set_sample_format',
        endpointId: 'device-a',
        sampleFormat: 'float32',
      });
    expect(port.getSnapshot).toHaveBeenCalledTimes(1);
    expect(port.mutate).not.toHaveBeenCalled();
    expect(time.pending()).toBe(0);
    unsubscribe();
  },
);
