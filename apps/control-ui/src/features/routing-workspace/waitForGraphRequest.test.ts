import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { monotonicClock, timerScheduler } from '../../core/adapters/browser/timers';
import { waitForGraphRequest } from '../../core/application/workspace/waitForGraphRequest';
import { ControlPortError } from '../../core/domain/control/errors';
import type { GraphRuntimeStatus } from '../../core/domain/control/graph';

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  expect(vi.getTimerCount()).toBe(0);
  vi.useRealTimers();
});
function fixture(permission = true, accepted = true) {
  const request = Promise.withResolvers<string>();
  const signal = new AbortController();
  const status: GraphRuntimeStatus = {
    protocolVersion: 1,
    activeRevision: 1,
    submittedRevision: 1,
  };
  if (permission) status.pendingReason = 'awaiting_microphone_permission';
  const port = {
    getGraphStatus: vi.fn().mockResolvedValue(status),
    isOperationPending: vi.fn().mockResolvedValue(accepted),
  };
  const waiting = vi.fn();
  const result = waitForGraphRequest(
    request.promise,
    { port, operationId: 'our-operation', signal: signal.signal, timeoutMs: 10_000 },
    waiting,
    (code) => new Error(code),
    timerScheduler,
    monotonicClock,
  );
  const outcome = result.then(
    (value) => value,
    (error: unknown) => error,
  );
  return { request, signal, status, port, waiting, outcome };
}

it.each([
  [false, true],
  [true, false],
])('keeps the 10s bound with permission=%s accepted=%s', async (permission, accepted) => {
  const f = fixture(permission, accepted);
  await vi.advanceTimersByTimeAsync(9_999);
  expect(f.waiting).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(await f.outcome).toEqual(new Error('graph_apply_timeout'));
  if (permission) expect(f.port.isOperationPending).toHaveBeenCalledWith('our-operation');
});

it('extends only the accepted operation while the engine reports microphone consent', async () => {
  const f = fixture();
  await vi.advanceTimersByTimeAsync(60_000);
  expect(f.waiting).toHaveBeenLastCalledWith(true);
  expect(f.port.isOperationPending).toHaveBeenCalledWith('our-operation');
  f.request.resolve('applied');
  await expect(f.outcome).resolves.toBe('applied');
  expect(f.waiting).toHaveBeenLastCalledWith(false);
});

it.each(['cleared', 'failed', 'hung'] as const)(
  'restores the short bound when the permission observation is %s',
  async (mode) => {
    const f = fixture();
    await vi.advanceTimersByTimeAsync(20_000);
    if (mode === 'cleared') delete f.status.pendingReason;
    if (mode === 'failed') f.port.getGraphStatus.mockRejectedValue(new Error('offline'));
    if (mode === 'hung') f.port.getGraphStatus.mockReturnValue(new Promise(() => {}));
    await vi.advanceTimersByTimeAsync(10_200);
    expect(await f.outcome).toEqual(new Error('graph_apply_timeout'));
    expect(f.waiting).toHaveBeenLastCalledWith(false);
  },
);

it('bounds an unanswered consent dialog even with fresh permission observations', async () => {
  const f = fixture();
  await vi.advanceTimersByTimeAsync(130_000);
  expect(await f.outcome).toEqual(new Error('graph_apply_timeout'));
});

it('ends promptly on denial and clears the waiting state', async () => {
  const f = fixture();
  await vi.advanceTimersByTimeAsync(20_000);
  const denial = new ControlPortError(403, { code: 'audio_permission_denied', parameters: {} });
  f.request.reject(denial);
  expect(await f.outcome).toBe(denial);
  expect(f.waiting).toHaveBeenLastCalledWith(false);
});

it('ignores a late status and completion after cancellation', async () => {
  const f = fixture(false);
  const observation = Promise.withResolvers<GraphRuntimeStatus>();
  f.port.getGraphStatus.mockReturnValue(observation.promise);
  await vi.advanceTimersByTimeAsync(200);
  f.signal.abort(new Error('disposed'));
  expect(await f.outcome).toEqual(new Error('disposed'));
  observation.resolve({ ...f.status, pendingReason: 'awaiting_microphone_permission' });
  f.request.resolve('late');
  await vi.advanceTimersByTimeAsync(0);
  expect(f.port.isOperationPending).not.toHaveBeenCalled();
  expect(f.waiting).not.toHaveBeenCalled();
});

it('handles an already aborted owner without an observer or UI callback', async () => {
  const signal = AbortSignal.abort(new Error('already disposed'));
  await expect(
    waitForGraphRequest(
      Promise.resolve('late'),
      { port: { getGraphStatus: vi.fn() }, operationId: 'old', timeoutMs: 10_000, signal },
      undefined,
      (code) => new Error(code),
      timerScheduler,
      monotonicClock,
    ),
  ).rejects.toThrow('already disposed');
});

it('does not renew or publish after cancellation during a permission receipt lookup', async () => {
  const f = fixture();
  const receipt = Promise.withResolvers<boolean>();
  f.port.isOperationPending.mockReturnValue(receipt.promise);
  await vi.advanceTimersByTimeAsync(200);
  expect(f.port.isOperationPending).toHaveBeenCalledOnce();
  f.signal.abort(new Error('disposed'));
  expect(await f.outcome).toEqual(new Error('disposed'));
  receipt.resolve(true);
  await vi.advanceTimersByTimeAsync(0);
  expect(f.waiting).not.toHaveBeenCalled();
  expect(f.port.getGraphStatus).toHaveBeenCalledOnce();
});
