import { expect, it, vi } from 'vitest';
import { fakeScheduler } from '../../../test-support/fakeScheduler';
import { RuntimeConnection } from './RuntimeConnection';

it('observes late readiness through the injected schedule without launching twice', async () => {
  const time = fakeScheduler();
  const port = {
    connect: vi.fn(async () => 'unavailable' as const),
    status: vi.fn(async () => 'ready' as const),
  };
  const owner = new RuntimeConnection(time);
  await owner.connect();
  expect(port.connect).not.toHaveBeenCalled();
  owner.start(port);
  await Promise.resolve();
  time.advance(1999);
  expect(port.status).not.toHaveBeenCalled();
  time.advance(1);
  await Promise.resolve();
  expect(owner.snapshot()).toMatchObject({ state: 'ready', opened: true });
  expect(port.connect).toHaveBeenCalledTimes(1);
  owner.dispose();
  expect(time.pending()).toBe(0);
});

it.each(['connecting', 'ready'] as const)(
  'disposal during %s publication sends no later health request',
  async (state) => {
    const time = fakeScheduler();
    const port = { connect: vi.fn(async () => 'ready' as const), status: vi.fn() };
    const owner = new RuntimeConnection(time);
    const unsubscribe = owner.subscribe(() => {
      if (owner.snapshot().state === state) owner.dispose();
    });
    owner.start(port);
    await Promise.resolve();
    expect(port.connect).toHaveBeenCalledTimes(state === 'connecting' ? 0 : 1);
    expect(time.pending()).toBe(0);
    time.advance(2000);
    expect(port.status).not.toHaveBeenCalled();
    unsubscribe();
  },
);

it('ignores an already queued health callback after disposal', async () => {
  let health: (() => void) | undefined;
  const scheduler = {
    after: vi.fn((_delay: number, run: () => void) => {
      health = run;
      return vi.fn();
    }),
  };
  const port = { connect: vi.fn(async () => 'ready' as const), status: vi.fn() };
  const owner = new RuntimeConnection(scheduler);
  owner.start(port);
  await Promise.resolve();
  expect(health).toBeTypeOf('function');
  owner.dispose();
  health?.();
  expect(port.status).not.toHaveBeenCalled();
});
