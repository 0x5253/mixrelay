import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { fakeScheduler } from '../../../test-support/fakeScheduler';
import { timerScheduler } from '../../adapters/browser/timers';
import type { ControlStateSnapshot } from '../../domain/control/state';
import type { StateListener } from '../../ports/control/TelemetryPort';
import { waitForPushedRevision } from './waitForPushedRevision';

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());
function snapshot(activeRevision: number, submittedRevision = 2): ControlStateSnapshot {
  return {
    generation: 1,
    graph: { protocolVersion: 1, activeRevision, submittedRevision },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
    controls: null,
    editors: [],
  };
}
function port(initial?: ControlStateSnapshot, incarnation = 'engine') {
  let listener!: StateListener;
  const close = vi.fn();
  const getGraphStatus = vi.fn().mockResolvedValue(snapshot(1).graph);
  const subscribeState = vi.fn((receive: StateListener) => {
    listener = receive;
    receive(initial, incarnation);
    return close;
  });
  return {
    getGraphStatus,
    subscribeState,
    close,
    receive: (state?: ControlStateSnapshot, engine = incarnation) => listener(state, engine),
  };
}

it('waits for pushed activation without polling and removes its subscription and deadline', async () => {
  const source = port(snapshot(1));
  const waiting = waitForPushedRevision(
    source,
    2,
    new AbortController().signal,
    1000,
    timerScheduler,
  );
  await vi.advanceTimersByTimeAsync(500);
  expect(source.getGraphStatus).not.toHaveBeenCalled();
  source.receive(snapshot(2));
  await waiting;
  expect(source.close).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});

it('fences a late fallback reply after push arrives', async () => {
  const source = port();
  const pending = Promise.withResolvers<ControlStateSnapshot['graph']>();
  source.getGraphStatus.mockReturnValueOnce(pending.promise);
  const waiting = waitForPushedRevision(
    source,
    2,
    new AbortController().signal,
    1000,
    timerScheduler,
  );
  await vi.advanceTimersByTimeAsync(0);
  source.receive(snapshot(1));
  pending.resolve(snapshot(99, 99).graph);
  await Promise.resolve();
  await Promise.resolve();
  source.receive(snapshot(2));
  await waiting;
  expect(source.getGraphStatus).toHaveBeenCalledOnce();
});

it('ignores late failed fallback reads and late state callbacks after completion', async () => {
  const source = port();
  const pending = Promise.withResolvers<ControlStateSnapshot['graph']>();
  source.getGraphStatus.mockReturnValueOnce(pending.promise);
  const waiting = waitForPushedRevision(
    source,
    2,
    new AbortController().signal,
    1000,
    timerScheduler,
  );
  await vi.advanceTimersByTimeAsync(0);
  source.receive(snapshot(1));
  pending.reject(new Error('retired read'));
  await vi.advanceTimersByTimeAsync(0);
  source.receive(snapshot(2));
  await waiting;
  source.receive(snapshot(99, 99));
  expect(source.close).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});

it('supports a polling-only port and cancellation while a fallback is pending', async () => {
  const source = { getGraphStatus: vi.fn().mockResolvedValue(snapshot(2).graph) };
  const waiting = waitForPushedRevision(
    source,
    2,
    new AbortController().signal,
    1000,
    timerScheduler,
  );
  await vi.advanceTimersByTimeAsync(0);
  await waiting;
  const controller = new AbortController();
  const pending = Promise.withResolvers<ControlStateSnapshot['graph']>();
  source.getGraphStatus.mockReturnValueOnce(pending.promise);
  const cancelled = waitForPushedRevision(source, 2, controller.signal, 1000, timerScheduler);
  const rejected = expect(cancelled).rejects.toThrow('cancelled');
  await vi.advanceTimersByTimeAsync(0);
  controller.abort(new Error('cancelled'));
  await rejected;
  pending.resolve(snapshot(2).graph);
  await vi.advanceTimersByTimeAsync(0);
  expect(vi.getTimerCount()).toBe(0);
});

it('falls back only while push is unavailable and completes an immediate snapshot', async () => {
  const source = port();
  source.getGraphStatus
    .mockResolvedValueOnce(snapshot(1).graph)
    .mockResolvedValueOnce(snapshot(2).graph);
  const waiting = waitForPushedRevision(
    source,
    2,
    new AbortController().signal,
    1000,
    timerScheduler,
  );
  await vi.advanceTimersByTimeAsync(200);
  await waiting;
  expect(source.getGraphStatus).toHaveBeenCalledTimes(2);
  const ready = port(snapshot(2));
  await waitForPushedRevision(ready, 2, new AbortController().signal, 1000, timerScheduler);
  expect(ready.close).toHaveBeenCalledOnce();
});

it('resumes a fresh revision from the same engine after a stream interruption without polling', async () => {
  const source = port(snapshot(1));
  const waiting = waitForPushedRevision(
    source,
    2,
    new AbortController().signal,
    10_000,
    timerScheduler,
  );
  source.receive();
  await vi.advanceTimersByTimeAsync(7000);
  expect(source.getGraphStatus).not.toHaveBeenCalled();
  expect(source.close).not.toHaveBeenCalled();
  source.receive(snapshot(2));
  await waiting;
  expect(source.close).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});

it('rejects replacement even when its revision matches and retains the original deadline on loss', async () => {
  const source = port(snapshot(1));
  const waiting = waitForPushedRevision(
    source,
    2,
    new AbortController().signal,
    1000,
    timerScheduler,
  );
  const replaced = expect(waiting).rejects.toThrow('engine_replaced');
  source.receive();
  source.receive(snapshot(2), 'replacement');
  await replaced;
  const lost = port(snapshot(1));
  const expires = waitForPushedRevision(
    lost,
    2,
    new AbortController().signal,
    1000,
    timerScheduler,
  );
  const expired = expect(expires).rejects.toThrow('graph_apply_timeout');
  await vi.advanceTimersByTimeAsync(750);
  lost.receive();
  await vi.advanceTimersByTimeAsync(250);
  await expired;
  expect(lost.getGraphStatus).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});

it('rejects revision supersession, deadline, cancellation and fallback errors', async () => {
  const stale = port(snapshot(3, 3));
  await expect(
    waitForPushedRevision(stale, 2, new AbortController().signal, 1000, timerScheduler),
  ).rejects.toThrow('graph_revision_stale');
  const timeout = waitForPushedRevision(
    port(snapshot(1)),
    2,
    new AbortController().signal,
    1000,
    timerScheduler,
  );
  const expired = expect(timeout).rejects.toThrow('graph_apply_timeout');
  await vi.advanceTimersByTimeAsync(1000);
  await expired;
  const cancelled = new AbortController();
  cancelled.abort(new Error('cancelled'));
  await expect(
    waitForPushedRevision(port(), 2, cancelled.signal, 1000, timerScheduler),
  ).rejects.toThrow('cancelled');
  const failed = port();
  failed.getGraphStatus.mockRejectedValue(new Error('offline'));
  const rejected = expect(
    waitForPushedRevision(failed, 2, new AbortController().signal, 1000, timerScheduler),
  ).rejects.toThrow('offline');
  await vi.advanceTimersByTimeAsync(0);
  await rejected;
  expect(vi.getTimerCount()).toBe(0);
});

it('expires at the original deadline however often the stream interrupts', async () => {
  const scheduler = fakeScheduler();
  const source = port(snapshot(1));
  const waiting = waitForPushedRevision(source, 2, new AbortController().signal, 1000, scheduler);
  const outcome = waiting.catch((error: unknown) => error);
  scheduler.advance(600);
  source.receive(undefined);
  source.receive(snapshot(1));
  scheduler.advance(399);
  expect(source.close).not.toHaveBeenCalled();
  scheduler.advance(1);
  await expect(outcome).resolves.toEqual(new Error('graph_apply_timeout'));
  expect(source.close).toHaveBeenCalledOnce();
  expect(scheduler.pending()).toBe(0);
});
