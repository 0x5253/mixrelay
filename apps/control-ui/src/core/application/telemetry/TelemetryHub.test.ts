import { expect, it, vi } from 'vitest';
import { fakeScheduler } from '../../../test-support/fakeScheduler';
import type { ControlStateSnapshot, ControlTelemetry } from '../../domain/control/state';
import {
  TELEMETRY_RETRY_MS,
  TELEMETRY_SILENCE_MS,
  TelemetryHub,
  type TelemetryStreamListener,
} from './TelemetryHub';

function state(generation = 1): ControlStateSnapshot {
  return {
    generation,
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
    graph: { protocolVersion: 1, activeRevision: 2, submittedRevision: 2 },
    controls: null,
    editors: [],
  };
}

function packet(generation = 1, engineIncarnation = 'a'): ControlTelemetry {
  return {
    engineIncarnation,
    sequence: generation,
    state: state(generation),
    meters: { protocolVersion: 1, revision: 2, nodes: [] },
  };
}

/** A source whose opens resolve only when the test says so. */
function source() {
  const attempts: { listener: TelemetryStreamListener; close: () => void }[] = [];
  const opens: PromiseWithResolvers<() => void>[] = [];
  const open = vi.fn((listener: TelemetryStreamListener) => {
    const opened = Promise.withResolvers<() => void>();
    attempts.push({ listener, close: vi.fn<() => void>() });
    opens.push(opened);
    return opened.promise;
  });
  const scheduler = fakeScheduler();
  const hub = new TelemetryHub(open, scheduler);
  const accept = async (index: number) => {
    const close = attempts[index]?.close;
    if (close) opens[index]?.resolve(close);
    await Promise.resolve();
    await Promise.resolve();
  };
  return { attempts, opens, open, scheduler, hub, accept };
}

it('shares one stream across state and meter consumers and replays state to late ones', async () => {
  const { hub, open, attempts, accept, scheduler } = source();
  const receive = vi.fn();
  const stopState = hub.subscribeState(receive);
  expect(receive).toHaveBeenCalledWith(undefined, undefined);
  const meters = { window: vi.fn(), ended: vi.fn() };
  const subscribing = hub.subscribeMeters(meters);
  await accept(0);
  const stopMeters = await subscribing;
  attempts[0]?.listener.window(packet());
  expect(open).toHaveBeenCalledOnce();
  expect(receive).toHaveBeenLastCalledWith(state(), 'a');
  expect(meters.window).toHaveBeenCalledWith(packet());
  const late = vi.fn();
  const stopLate = hub.subscribeState(late);
  expect(late).toHaveBeenCalledWith(state(), 'a');
  // An older generation of the same engine never replaces a newer snapshot.
  attempts[0]?.listener.window(packet(2));
  attempts[0]?.listener.window(packet(1));
  expect(hub.current()?.generation).toBe(2);
  stopState();
  stopMeters();
  expect(attempts[0]?.close).not.toHaveBeenCalled();
  stopLate();
  expect(attempts[0]?.close).toHaveBeenCalledOnce();
  expect(hub.current()).toBeUndefined();
  expect(scheduler.pending()).toBe(0);
});

it('invalidates cached state when the engine is replaced', async () => {
  const { hub, attempts, accept } = source();
  const receive = vi.fn();
  hub.subscribeState(receive);
  await accept(0);
  attempts[0]?.listener.window(packet(8));
  attempts[0]?.listener.window(packet(1, 'replacement'));
  expect(receive.mock.calls.slice(-2)).toEqual([
    [undefined, 'replacement'],
    [state(1), 'replacement'],
  ]);
  expect(hub.current()?.generation).toBe(1);
});

it('treats silence as an ended stream: meters end, state waits and reopens after the retry delay', async () => {
  const { hub, open, attempts, accept, scheduler } = source();
  const receive = vi.fn();
  hub.subscribeState(receive);
  const meters = { window: vi.fn(), ended: vi.fn() };
  const subscribing = hub.subscribeMeters(meters);
  await accept(0);
  await subscribing;
  attempts[0]?.listener.window(packet());
  scheduler.advance(TELEMETRY_SILENCE_MS - 1);
  attempts[0]?.listener.window(packet(2));
  scheduler.advance(TELEMETRY_SILENCE_MS - 1);
  expect(meters.ended).not.toHaveBeenCalled();
  scheduler.advance(1);
  expect(meters.ended).toHaveBeenCalledOnce();
  expect(attempts[0]?.close).toHaveBeenCalledOnce();
  expect(receive).toHaveBeenLastCalledWith(undefined, 'a');
  scheduler.advance(TELEMETRY_RETRY_MS - 1);
  expect(open).toHaveBeenCalledOnce();
  scheduler.advance(1);
  expect(open).toHaveBeenCalledTimes(2);
  // The retired attempt can no longer deliver or end the reopened stream.
  attempts[0]?.listener.window(packet(99));
  attempts[0]?.listener.ended();
  expect(hub.current()).toBeUndefined();
  await accept(1);
  attempts[1]?.listener.window(packet(3));
  expect(hub.current()?.generation).toBe(3);
  expect(meters.window).toHaveBeenCalledTimes(2);
});

it('reconnects after the stream reports its end', async () => {
  const { hub, open, attempts, accept, scheduler } = source();
  hub.subscribeState(vi.fn());
  await accept(0);
  attempts[0]?.listener.ended();
  scheduler.advance(TELEMETRY_RETRY_MS);
  expect(open).toHaveBeenCalledTimes(2);
});

it('rejects a refused meter subscription without ending it and retries for state consumers', async () => {
  const { hub, open, opens, scheduler } = source();
  hub.subscribeState(vi.fn());
  const meters = { window: vi.fn(), ended: vi.fn() };
  const subscribing = hub.subscribeMeters(meters);
  opens[0]?.reject(new Error('offline'));
  await expect(subscribing).rejects.toThrow('offline');
  expect(meters.ended).not.toHaveBeenCalled();
  scheduler.advance(TELEMETRY_RETRY_MS);
  expect(open).toHaveBeenCalledTimes(2);
  // A refused retry schedules the next one.
  opens[1]?.reject(new Error('still offline'));
  await Promise.resolve();
  await Promise.resolve();
  scheduler.advance(TELEMETRY_RETRY_MS);
  expect(open).toHaveBeenCalledTimes(3);
});

it('closes a stream that opens after its last consumer left and ignores its late failure', async () => {
  const { hub, open, attempts, opens, accept, scheduler } = source();
  const stop = hub.subscribeState(vi.fn());
  stop();
  await accept(0);
  expect(attempts[0]?.close).toHaveBeenCalledOnce();
  const again = hub.subscribeState(vi.fn());
  again();
  attempts[1]?.listener.ended();
  opens[1]?.reject(new Error('retired opening'));
  await Promise.resolve();
  scheduler.advance(TELEMETRY_RETRY_MS);
  expect(open).toHaveBeenCalledTimes(2);
  expect(scheduler.pending()).toBe(0);
});

it('a meter consumer that leaves while the stream opens keeps nothing open', async () => {
  const { hub, opens } = source();
  const subscribing = hub.subscribeMeters({ window: vi.fn(), ended: vi.fn() });
  opens[0]?.reject(new Error('offline'));
  await expect(subscribing).rejects.toThrow('offline');
  expect(hub.current()).toBeUndefined();
});
