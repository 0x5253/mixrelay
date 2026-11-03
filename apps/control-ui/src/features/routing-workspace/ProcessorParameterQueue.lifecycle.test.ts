import { expect, it, vi } from 'vitest';
import type { ProcessorParameterUpdateResponse } from '../../core/domain/control/processors';
import { ProcessorParameterQueue } from './ProcessorParameterQueue';

const update = { expectedRevision: 1, nodeId: 'processor-a', parameterId: 1, value: 0.5 };
const response: ProcessorParameterUpdateResponse = {
  protocolVersion: 1,
  nodeId: update.nodeId,
  parameterId: 1,
  value: 0.5,
};
function fixture() {
  const send = vi.fn().mockResolvedValue(response);
  const change = vi.fn();
  const warning = vi.fn();
  const queue = new ProcessorParameterQueue(
    { setProcessorParameter: send },
    { change, persistenceWarning: warning },
  );
  return { send, change, warning, queue };
}

it('keeps a closed session inert for explicit retries, discards, and new edits', async () => {
  const f = fixture();
  f.send.mockRejectedValueOnce(new Error('failed update'));
  f.queue.publish(update);
  await expect(f.queue.whenIdle()).rejects.toThrow();
  f.queue.close();
  const notifications = f.change.mock.calls.length;
  f.queue.retryFailed();
  f.queue.discardFailures();
  f.queue.publish({ ...update, value: 0.9 });
  await expect(f.queue.whenIdle()).resolves.toBeUndefined();
  expect(f.send).toHaveBeenCalledTimes(1);
  expect(f.change).toHaveBeenCalledTimes(notifications);
});

it('never replaces a newer pending control value with an explicit retry of its older failure', async () => {
  const f = fixture();
  f.send.mockRejectedValueOnce(new Error('first value failed'));
  f.queue.publish(update);
  await expect(f.queue.whenIdle()).rejects.toThrow();
  const other = Promise.withResolvers<ProcessorParameterUpdateResponse>();
  f.send.mockReturnValueOnce(other.promise);
  f.queue.publish({ ...update, parameterId: 2 });
  f.queue.publish({ ...update, value: 0.9 });
  f.queue.retryFailed();
  other.resolve({ ...response, parameterId: 2 });
  await f.queue.whenIdle();
  expect(f.send.mock.calls).toEqual([
    ['processor-a', 1, 0.5, 1],
    ['processor-a', 2, 0.5, 1],
    ['processor-a', 1, 0.9, 1],
  ]);
  expect(f.change).toHaveBeenLastCalledWith(false, []);
});

it('drains an edit published by the idle notification without losing the next owner task', async () => {
  const f = fixture();
  let published = false;
  f.change.mockImplementation((busy: boolean) => {
    if (!busy && !published) {
      published = true;
      f.queue.publish({ ...update, value: 0.9 });
    }
  });
  f.queue.publish(update);
  await f.queue.whenIdle();
  expect(f.send.mock.calls).toEqual([
    ['processor-a', 1, 0.5, 1],
    ['processor-a', 1, 0.9, 1],
  ]);
  expect(f.change).toHaveBeenLastCalledWith(false, []);
});
