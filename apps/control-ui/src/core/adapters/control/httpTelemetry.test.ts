import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { packedTelemetry } from '../../../test-support/controlTelemetry';
import { timerScheduler } from '../browser/timers';
import { openHttpTelemetry } from './httpTelemetry';

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

function stream(engine: string | null = 'a') {
  let feed!: ReadableStreamDefaultController<Uint8Array>;
  const cancel = vi.fn();
  const body = new ReadableStream<Uint8Array>({
    start: (controller) => {
      feed = controller;
    },
    cancel,
  });
  const headers = new Headers();
  if (engine) headers.set('X-MixRelay-Engine', engine);
  const request = vi.fn<typeof fetch>().mockResolvedValue(new Response(body, { headers }));
  const listener = { window: vi.fn(), ended: vi.fn() };
  const open = () => openHttpTelemetry('/engine', request, listener, timerScheduler);
  const send = (data: unknown) =>
    feed.enqueue(new TextEncoder().encode(`event: telemetry\ndata: ${JSON.stringify(data)}\n\n`));
  return { feed, cancel, request, listener, open, send };
}

it('parses fragmented and batched SSE, waits for the full snapshot and closes without an end notification', async () => {
  const source = stream();
  const opening = source.open();
  const event = new TextEncoder().encode(
    `: heartbeat\n\ndata: ${JSON.stringify(packedTelemetry())}\n\n`,
  );
  source.feed.enqueue(event.slice(0, 17));
  source.feed.enqueue(event.slice(17));
  const close = await opening;
  expect(source.listener.window).toHaveBeenCalledOnce();
  expect(source.request).toHaveBeenCalledWith('/engine/api/v1/telemetry', {
    signal: expect.any(AbortSignal),
  });
  source.send(packedTelemetry());
  source.send(packedTelemetry());
  await vi.advanceTimersByTimeAsync(0);
  expect(source.listener.window).toHaveBeenCalledTimes(3);
  close();
  await vi.advanceTimersByTimeAsync(0);
  expect(source.cancel).toHaveBeenCalledOnce();
  expect(source.listener.ended).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});

it.each(['missing', 'identity', 'oversized', 'malformed', 'eof'])(
  'rejects %s before the initial snapshot',
  async (failure) => {
    const source = stream();
    const rejected = expect(source.open()).rejects.toThrow();
    if (failure === 'missing') source.send({ ...packedTelemetry(), state: null });
    if (failure === 'identity') source.send({ ...packedTelemetry(), engineIncarnation: 'b' });
    if (failure === 'oversized') source.feed.enqueue(new Uint8Array(4 * 1024 * 1024 + 1));
    if (failure === 'malformed') source.feed.enqueue(new TextEncoder().encode('data: invalid\n\n'));
    if (failure === 'eof') source.feed.close();
    await rejected;
    expect(source.listener.ended).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  },
);

it.each([true, false])('ends a started stream on EOF or decode failure (EOF: %s)', async (eof) => {
  const error = vi.spyOn(console, 'error').mockImplementation(() => undefined);
  const source = stream(null);
  const opening = source.open();
  source.send(packedTelemetry());
  await opening;
  if (eof) source.feed.close();
  else source.send({});
  await vi.advanceTimersByTimeAsync(0);
  expect(source.listener.ended).toHaveBeenCalledOnce();
  // Why the stream ended is logged once, including the path of an unreadable event.
  expect(error).toHaveBeenCalledOnce();
  if (!eof) expect(error.mock.calls[0]?.[1]).toMatchObject({ path: 'telemetry.engineIncarnation' });
  error.mockRestore();
});

it('keeps metering through a started stream whose state snapshot is unreadable', async () => {
  const error = vi.spyOn(console, 'error').mockImplementation(() => undefined);
  const source = stream(null);
  const opening = source.open();
  source.send(packedTelemetry());
  await opening;
  source.send({ ...packedTelemetry(), sequence: 2, state: { generation: -1 } });
  source.send({ ...packedTelemetry(), sequence: 3, state: { generation: -1 } });
  await vi.advanceTimersByTimeAsync(0);
  expect(source.listener.window).toHaveBeenCalledTimes(3);
  expect(source.listener.window.mock.lastCall?.[0].state).toBeUndefined();
  expect(source.listener.ended).not.toHaveBeenCalled();
  expect(error).toHaveBeenCalledOnce();
  error.mockRestore();
});

it('starts metering when the opening snapshot itself is unreadable', async () => {
  const error = vi.spyOn(console, 'error').mockImplementation(() => undefined);
  const source = stream(null);
  const opening = source.open();
  source.send({ ...packedTelemetry(), state: { generation: -1 } });
  await opening;
  source.send({ ...packedTelemetry(), sequence: 2, state: { generation: -1 } });
  await vi.advanceTimersByTimeAsync(0);
  expect(source.listener.window).toHaveBeenCalledTimes(2);
  expect(source.listener.window.mock.calls[0]?.[0].state).toBeUndefined();
  expect(source.listener.ended).not.toHaveBeenCalled();
  expect(error).toHaveBeenCalledOnce();
  error.mockRestore();
});

it.each([new Response(null, { status: 503 }), new Response(null)])(
  'rejects unavailable HTTP or body',
  async (response) => {
    const request = vi.fn<typeof fetch>().mockResolvedValue(response);
    await expect(
      openHttpTelemetry('', request, { window: vi.fn(), ended: vi.fn() }, timerScheduler),
    ).rejects.toThrow('telemetry unavailable');
  },
);

it('aborts a stalled open at its deadline and retires a read resolved during cancellation', async () => {
  const request = vi.fn<typeof fetch>(
    (_url, init) =>
      new Promise((_resolve, reject) => {
        init?.signal?.addEventListener('abort', () => reject(new Error('aborted')));
      }),
  );
  const rejected = expect(
    openHttpTelemetry('', request, { window: vi.fn(), ended: vi.fn() }, timerScheduler),
  ).rejects.toThrow('aborted');
  await vi.advanceTimersByTimeAsync(5000);
  await rejected;
  const source = stream();
  const opening = source.open();
  source.send(packedTelemetry());
  const close = await opening;
  source.send(packedTelemetry());
  close();
  await vi.advanceTimersByTimeAsync(0);
  expect(source.listener.window).toHaveBeenCalledOnce();
});
