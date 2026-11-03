import { expect, it, vi } from 'vitest';
import type { ProductUpdate } from '../../../core/domain/updates';
import { fakeScheduler } from '../../../test-support/fakeScheduler';
import { UpdatesSession } from './UpdatesSession';

it('retires replayed startup work and cancels the daily check with its owner', async () => {
  const time = fakeScheduler();
  const port = { check: vi.fn(async () => []), openDownload: vi.fn() };
  const session = new UpdatesSession(port, time);
  session.start();
  session.dispose();
  session.start();
  await Promise.resolve();
  await Promise.resolve();
  expect(port.check).toHaveBeenCalledTimes(1);
  time.advance(24 * 60 * 60 * 1000 - 1);
  expect(port.check).toHaveBeenCalledTimes(1);
  time.advance(1);
  expect(port.check).toHaveBeenCalledTimes(2);
  session.dispose();
  expect(time.pending()).toBe(0);
});

it('restarts a pending check and keeps an old check or download failure out of the new owner', async () => {
  const time = fakeScheduler();
  const checking = Promise.withResolvers<ProductUpdate[]>();
  const opening = Promise.withResolvers<void>();
  const port = {
    check: vi.fn().mockReturnValueOnce(checking.promise).mockResolvedValue([]),
    openDownload: vi.fn(() => opening.promise),
  };
  const session = new UpdatesSession(port, time);
  session.start();
  await Promise.resolve();
  expect(session.snapshot().checking).toBe(true);
  session.dispose();
  session.start();
  await Promise.resolve();
  await Promise.resolve();
  expect(session.snapshot()).toMatchObject({ checked: true, checking: false, failed: false });
  checking.reject(new Error('old check failed'));
  await Promise.resolve();
  await Promise.resolve();
  expect(session.snapshot().failed).toBe(false);

  const download = session.openDownload('mix-relay', 'https://downloads.example/app.pkg');
  expect(session.snapshot().opening).not.toBeNull();
  session.dispose();
  session.start();
  await Promise.resolve();
  await Promise.resolve();
  opening.reject(new Error('old download failed'));
  await download;
  expect(session.snapshot()).toMatchObject({ checked: true, opening: null, openFailed: false });
  expect(port.check).toHaveBeenCalledTimes(3);
  expect(port.openDownload).toHaveBeenCalledOnce();
  session.dispose();
  expect(time.pending()).toBe(0);
});

it('does not create the daily timer when startup publication disposes its owner', async () => {
  const time = fakeScheduler();
  const port = { check: vi.fn(async () => []), openDownload: vi.fn() };
  const owner = new UpdatesSession(port, time);
  const unsubscribe = owner.subscribe(() => owner.dispose());
  owner.start();
  await Promise.resolve();
  expect(port.check).not.toHaveBeenCalled();
  expect(time.pending()).toBe(0);
  unsubscribe();
});

it.each(['check', 'download'] as const)(
  'disposal during %s publication prevents its native request',
  async (operation) => {
    const time = fakeScheduler();
    const port = { check: vi.fn(async () => []), openDownload: vi.fn() };
    const owner = new UpdatesSession(port, time);
    owner.start();
    await Promise.resolve();
    await Promise.resolve();
    const unsubscribe = owner.subscribe(() => {
      if (owner.snapshot().checking || owner.snapshot().opening) owner.dispose();
    });
    if (operation === 'check') await owner.refresh();
    else await owner.openDownload('mix-relay', 'https://downloads.example/app.pkg');
    expect(port.check).toHaveBeenCalledOnce();
    expect(port.openDownload).not.toHaveBeenCalled();
    expect(time.pending()).toBe(0);
    unsubscribe();
  },
);

it('reports a current download failure and clears it on a successful retry', async () => {
  const time = fakeScheduler();
  const port = {
    check: vi.fn(async () => []),
    openDownload: vi
      .fn()
      .mockRejectedValueOnce(new Error('unavailable'))
      .mockResolvedValue(undefined),
  };
  const owner = new UpdatesSession(port, time);
  owner.start();
  await Promise.resolve();
  await Promise.resolve();
  await owner.openDownload('mix-relay', 'https://downloads.example/app.pkg');
  expect(owner.snapshot()).toMatchObject({ opening: null, openFailed: true });
  await owner.openDownload('mix-relay', 'https://downloads.example/app.pkg');
  expect(owner.snapshot()).toMatchObject({ opening: null, openFailed: false });
  expect(port.openDownload).toHaveBeenCalledTimes(2);
  owner.dispose();
  expect(time.pending()).toBe(0);
});
