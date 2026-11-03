import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { controlState, packedTelemetry } from '../../../test-support/controlTelemetry';
import type { ControlPort } from '../../ports/control/ControlPort';
import { HttpControlPort } from './HttpControlPort';
import { TauriControlPort } from './TauriControlPort';

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

function nativePort() {
  const invoke = vi.fn().mockResolvedValue(1);
  const channel = { onmessage: (_message: unknown) => {} };
  const port = new TauriControlPort(invoke, () => channel);
  const send = (state = controlState()) => {
    const metadata = new TextEncoder().encode(JSON.stringify(packedTelemetry(state)));
    const buffer = new ArrayBuffer((metadata.length + 7) & ~3);
    new DataView(buffer).setUint32(0, metadata.length, true);
    new Uint8Array(buffer, 4, metadata.length).set(metadata);
    channel.onmessage(buffer);
  };
  return { port, invoke, channel, send };
}

/** Reads are authoritative even while push is live: a pushed snapshot can predate a
 * mutation that just completed, so callers that need current state read the engine. */
async function readAll(port: ControlPort) {
  await port.getStatus().catch(() => undefined);
  await port.getGraphStatus().catch(() => undefined);
  await port.getNodeControls?.('a').catch(() => undefined);
  await port.getProcessorEditor('effect').catch(() => undefined);
}

function stateWithEditor() {
  const state = controlState();
  state.editors = [
    {
      protocolVersion: 1,
      nodeId: 'effect',
      state: 'open',
      width: null,
      height: null,
      resizable: false,
      failure: null,
    },
  ];
  return state;
}

it('native state and meters share one raw channel; reads still go to the engine', async () => {
  const source = nativePort();
  const receive = vi.fn();
  const stop = source.port.subscribeState(receive);
  const stopMeters = await source.port.subscribeMeters({ window: vi.fn(), ended: vi.fn() });
  const state = stateWithEditor();
  source.send(state);
  expect(receive).toHaveBeenLastCalledWith(state, 'a');
  await readAll(source.port);
  expect(source.invoke.mock.calls.map(([command]) => command)).toEqual([
    'control_subscribe_telemetry',
    'control_telemetry_credit',
    'control_get_status',
    'control_get_graph_status',
    'control_get_node_controls',
    'control_get_processor_editor',
  ]);
  source.invoke.mockRejectedValue(new Error('closed'));
  source.channel.onmessage(new ArrayBuffer(3));
  expect(receive).toHaveBeenLastCalledWith(undefined, 'a');
  source.channel.onmessage({});
  stop();
  stopMeters();
  await vi.advanceTimersByTimeAsync(0);
});

it('the HTTP dev port shares its SSE with meters; reads still go to the engine', async () => {
  let feed!: ReadableStreamDefaultController<Uint8Array>;
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      feed = controller;
    },
    cancel() {
      throw new Error('already closed');
    },
  });
  const request = vi.fn<typeof fetch>().mockResolvedValue(new Response(body));
  const port = new HttpControlPort('/engine', request);
  const stop = port.subscribeState(vi.fn());
  const meters = port.subscribeMeters({ window: vi.fn(), ended: vi.fn() });
  const state = stateWithEditor();
  feed.enqueue(new TextEncoder().encode(`data: ${JSON.stringify(packedTelemetry(state))}\n\n`));
  const stopMeters = await meters;
  expect(request).toHaveBeenCalledOnce();
  await readAll(port);
  expect(request.mock.calls.map(([url]) => String(url))).toEqual([
    '/engine/api/v1/telemetry',
    '/engine/api/v1/status',
    '/engine/api/v1/graph/status',
    '/engine/api/v1/graphs/a/nodes/controls',
    '/engine/api/v1/processors/effect/editor',
  ]);
  stop();
  stopMeters();
  await vi.advanceTimersByTimeAsync(0);
});
