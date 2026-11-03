import { expect, it, vi } from 'vitest';
import type { ControlStateSnapshot } from '../../domain/control/state';
import { TelemetryDecoder } from './telemetryCodec';

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
    controls: {
      revision: 2,
      activeRevision: 2,
      structureGeneration: 1,
      graphs: [{ id: 'a', nodes: [] }],
    },
    editors: [],
  };
}
function packed() {
  const samples = new Float32Array([0.5, 0.25, 0.75, 6, -12]);
  const metadata = {
    engineIncarnation: 'a',
    sequence: 1,
    state: state(),
    meters: {
      protocolVersion: 1,
      revision: 2,
      bindings: [
        { nodeId: 'n', portId: 'out', channels: 2, inputChannels: 1 },
        { nodeId: 'empty', portId: 'in', channels: 0, inputChannels: null },
      ],
      samples: btoa(String.fromCharCode(...new Uint8Array(samples.buffer))),
    },
  };
  const json = new TextEncoder().encode(
    JSON.stringify({ ...metadata, meters: { ...metadata.meters, samples: '' } }),
  );
  const offset = (json.length + 7) & ~3;
  const frame = new ArrayBuffer(offset + samples.byteLength);
  new DataView(frame).setUint32(0, json.length, true);
  new Uint8Array(frame, 4, json.length).set(json);
  new Float32Array(frame, offset).set(samples);
  return { metadata, frame };
}

it('decodes base64 SSE and raw ArrayBuffer identically, retaining actual dynamics', () => {
  const { metadata, frame } = packed();
  const decoded = new TelemetryDecoder().raw(frame);
  expect(decoded).toEqual(new TelemetryDecoder().sse(JSON.stringify(metadata)));
  expect(decoded.meters.nodes[0]).toEqual({
    nodeId: 'n',
    portId: 'out',
    peaks: [0.5, 0.25],
    dynamics: { inputPeaks: [0.75], gainReductionDb: 6, thresholdDb: -12 },
  });
  expect(decoded.state).toEqual(state());
});

it('reuses the bindings of a stream until they change and refuses a stream joined without them', () => {
  const { metadata } = packed();
  const decoder = new TelemetryDecoder();
  const first = decoder.sse(JSON.stringify(metadata));
  const { bindings: _omitted, ...meters } = metadata.meters;
  const steady = { ...metadata, sequence: 2, meters };
  expect(decoder.sse(JSON.stringify(steady)).meters.nodes).toEqual(first.meters.nodes);
  expect(() => new TelemetryDecoder().sse(JSON.stringify(steady))).toThrow(
    'meter bindings missing',
  );
});

it('rejects truncated headers, invalid bindings, excess samples and nonfinite values', () => {
  expect(() => new TelemetryDecoder().raw(new ArrayBuffer(3))).toThrow();
  const { metadata, frame } = packed();
  new DataView(frame).setUint32(0, frame.byteLength, true);
  expect(() => new TelemetryDecoder().raw(frame)).toThrow('invalid telemetry header');
  for (const channels of [-1, 0.5, 99, 0]) {
    const invalid = structuredClone(metadata);
    const binding = invalid.meters.bindings[0];
    if (binding) binding.channels = channels;
    expect(() => new TelemetryDecoder().sse(JSON.stringify(invalid))).toThrow();
  }
  metadata.meters.samples = btoa(
    String.fromCharCode(...new Uint8Array(new Float32Array([Number.NaN, 0, 0, 0, 0]).buffer)),
  );
  expect(() => new TelemetryDecoder().sse(JSON.stringify(metadata))).toThrow(
    'invalid meter samples',
  );
});

it('drops a state snapshot that breaks its contract and keeps the meters, reporting it once', () => {
  const { metadata } = packed();
  const report = vi.fn();
  const decoder = new TelemetryDecoder(report);
  const broken = { ...metadata, state: { ...state(), generation: 'two' } };
  const first = decoder.sse(JSON.stringify(broken));
  expect(first.state).toBeUndefined();
  expect(first.meters.nodes[0]?.peaks).toEqual([0.5, 0.25]);
  const { bindings: _omitted, ...meters } = metadata.meters;
  expect(decoder.sse(JSON.stringify({ ...broken, sequence: 2, meters })).state).toBeUndefined();
  expect(report).toHaveBeenCalledOnce();
  expect(report.mock.calls[0]?.[1]).toMatchObject({ path: 'telemetry.state.generation' });
  expect(decoder.sse(JSON.stringify({ ...metadata, sequence: 3, meters })).state).toEqual(state());
  expect(decoder.sse(JSON.stringify({ ...metadata, sequence: 4, meters, state: null })).state).toBe(
    null,
  );
});

it('reads a window the native channel already parsed, with the same state rule', () => {
  const report = vi.fn();
  const decoder = new TelemetryDecoder(report);
  const meters = { protocolVersion: 1, revision: 2, nodes: [] };
  const window = { engineIncarnation: 'a', sequence: 1, meters };
  expect(decoder.window(window)).toEqual({ ...window, state: undefined });
  expect(decoder.window({ ...window, state: state() }).state).toEqual(state());
  expect(decoder.window({ ...window, state: { generation: 1 } }).state).toBeUndefined();
  expect(report).toHaveBeenCalledOnce();
  expect(() => decoder.window({ ...window, meters: { ...meters, protocolVersion: 2 } })).toThrow(
    'telemetry window.meters.protocolVersion',
  );
});

it('reports to the console by default', () => {
  const error = vi.spyOn(console, 'error').mockImplementation(() => undefined);
  new TelemetryDecoder().window({
    engineIncarnation: 'a',
    sequence: 1,
    meters: { protocolVersion: 1, revision: 2, nodes: [] },
    state: {},
  });
  expect(error).toHaveBeenCalledWith('Telemetry state snapshot rejected', expect.anything());
  error.mockRestore();
});
