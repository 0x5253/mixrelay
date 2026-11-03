import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import {
  INTERRUPTIONS, parseArguments, parseReported, roundtripProblems, roundtripToleranceFrames, runAcceptance,
} from './macos-latency-acceptance.mjs';

const SOURCE = '11111111-1111-4111-8111-111111111111';
const ENDPOINT = '22222222-2222-4222-8222-222222222222';
const UID = `io.mixrelay.audio.endpoint.${ENDPOINT.replaceAll('-', '')}`;
const REPORTED = 480;

/** A measured, complete impulse report whose device total matches the measured mean. */
const probeReport = (overrides = {}) => ({
  output: { rate: 48000, latency: 0, safetyOffset: 0, streamLatency: 0 },
  input: { rate: 48000, latency: REPORTED, safetyOffset: 0, streamLatency: 0 },
  mode: 'impulse', measured: true, latencyFrames: { min: 470, max: 490, mean: 485.5 },
  reportedLatencyFrames: REPORTED, timestampDiscontinuities: { output: 0, input: 0 }, complete: true,
  ...overrides,
});

/** An engine that answers the control API in memory, plus the probe and signal hooks. */
function fakeEngine(t, hooks = {}) {
  const evidence = join(mkdtempSync(join(tmpdir(), 'mixrelay-latency-')), 'evidence');
  t.after(() => rmSync(join(evidence, '..'), { recursive: true, force: true }));
  const original = { revision: 3, nodes: [], edges: [] };
  const state = { graph: structuredClone(original), puts: [], time: 0 };
  const signals = new EventEmitter();
  signals.pid = 777;
  signals.raised = [];
  signals.kill = (pid, signal) => signals.raised.push([pid, signal]);
  const warnings = [];
  const reply = (status, json, id) => ({ status, body: json === undefined ? '' : JSON.stringify(json),
    headers: id ? { 'x-mixrelay-operation-result': id } : {} });
  async function request(path, { method = 'GET', headers = {}, body } = {}) {
    const id = headers['x-mixrelay-operation-request'];
    if (method === 'DELETE') return reply(200);
    if (path === '/api/v1/graph' && method === 'PUT') {
      const document = JSON.parse(body);
      state.puts.push(document);
      if (hooks.put) await hooks.put(document, state, signals);
      if (document.expectedGraphId !== 'g1') return reply(409, { parameters: { selectedGraphId: 'g1' } }, id);
      const { expectedGraphId: _, ...graph } = document;
      state.graph = graph;
      return reply(200, graph, id);
    }
    const routes = {
      '/api/v1/graphs': () => ({ activeId: 'g1', graphs: [{ id: 'g1' }], document: state.graph }),
      '/api/v1/graph': () => state.graph,
      '/api/v1/graph/status': () => ({ activeRevision: state.graph.revision }),
      '/api/v1/graphs/g1/layout': () => ({ layoutRevision: 1, layout: { positions: { 'latency-acceptance-source': { x: 1 } } } }),
      '/api/v1/status': () => ({ audioSession: { virtualEndpointSettings: { devices: [
        { spec: { id: ENDPOINT, capture: { channels: 2 } }, nativeDeviceIds: [UID] },
        { spec: { id: SOURCE, render: { channels: 2 } }, nativeDeviceIds: [] },
      ] } } }),
    };
    if (method === 'PUT') return reply(200, {}, id);
    return reply(200, routes[path](), id);
  }
  const routed = () => state.graph.edges.some(edge => edge.id === 'latency-acceptance-route');
  const io = {
    request,
    async reported(uid, signal) {
      assert.equal(uid, UID);
      hooks.reported?.(state, signals);
      if (signal?.aborted) throw signal.reason;
      return { latency: routed() ? REPORTED : 0, safety: 0, stream: 0 };
    },
    async roundtrip(playback, uid) {
      if (hooks.roundtrip) return hooks.roundtrip(playback, uid);
      assert.equal(uid, UID);
      assert.equal(playback, `io.mixrelay.audio.endpoint.${SOURCE.replaceAll('-', '')}`);
      return { status: 0, stdout: JSON.stringify(probeReport()) };
    },
    now: () => state.time,
    async delay(ms, signal) {
      if (signal?.aborted) throw new DOMException('The operation was aborted', 'AbortError');
      state.time += ms;
    },
    signals,
    warn: message => warnings.push(message),
  };
  const options = { sourceUid: SOURCE, sourceEndpoint: SOURCE, endpointId: ENDPOINT, evidence, roundtrip: true };
  const read = name => JSON.parse(readFileSync(join(evidence, name), 'utf8'));
  return { io, options, state, signals, warnings, original, evidence, read };
}

const listeners = signals => INTERRUPTIONS.map(name => signals.listenerCount(name));

test('settling is explicit, bounded and never the default', () => {
  const args = [SOURCE, ENDPOINT, '/tmp/latency-evidence', '--verify-installed-latency'];
  assert.equal(parseArguments(args).settleSeconds, 0);
  assert.equal(parseArguments([...args, '--roundtrip', '--settle-seconds=30']).settleSeconds, 30);
  for (const option of ['--settle-seconds=-1', '--settle-seconds=121', '--settle-seconds=NaN', '--settle-seconds=0.5'])
    assert.throws(() => parseArguments([...args, option]), /Invalid option/);
  assert.throws(() => parseArguments([...args, '--settle-seconds=30', '--settle-seconds=30']), /Invalid option/);
});

test('a settled measurement waits after activation and records its scope', async t => {
  const engine = fakeEngine(t, { reported(state) {
    if (state.graph.revision === 4) assert.ok(state.time >= 30000);
  } });
  engine.options.settleSeconds = 30;
  const summary = await runAcceptance(engine.options, engine.io);
  assert.equal(summary.settleSeconds, 30);
  assert.equal(engine.read('measured.json').settleSeconds, 30);
  assert.equal(engine.read('measured.json').roundtripToleranceFrames, 50);
});

test('interruption during settling still restores the graph', async t => {
  const engine = fakeEngine(t);
  engine.options.settleSeconds = 30;
  const delay = engine.io.delay;
  engine.io.delay = async (ms, signal) => {
    if (ms === 30000) engine.signals.emit('SIGINT', 'SIGINT');
    await delay(ms, signal);
  };
  await assert.rejects(runAcceptance(engine.options, engine.io), /Interrupted by SIGINT/);
  assert.deepEqual(engine.state.graph, { ...engine.original, revision: 5 });
  assert.equal(engine.read('restored.json').reported.latency, 0);
});

test('a complete run restores the graph and confirms the round trip', async t => {
  const engine = fakeEngine(t);
  const summary = await runAcceptance(engine.options, engine.io);
  assert.equal(summary.reportedInputLatencyFrames, REPORTED);
  assert.deepEqual(engine.state.graph, { ...engine.original, revision: 5 });
  assert.equal(engine.read('measured.json').roundtripToleranceFrames, 50);
  assert.equal(engine.read('restored.json').reported.latency, 0);
  assert.deepEqual(engine.signals.raised, []);
  assert.deepEqual(listeners(engine.signals), [0, 0, 0]);
});

const FOREIGN = 'com.example.foreign-loopback';
const baselineReport = (overrides = {}) => probeReport({
  input: { uid: FOREIGN, rate: 48000, latency: 0 },
  output: { uid: FOREIGN, rate: 48000, latency: 0 },
  reportedLatencyFrames: 0,
  latencyFrames: { mean: 768 },
  ...overrides,
});

test('a foreign source is measured before mutation and its hidden latency stays separate', async t => {
  const engine = fakeEngine(t, { roundtrip(playback, uid) {
    assert.equal(playback, FOREIGN);
    if (uid === FOREIGN) {
      assert.equal(engine.state.puts.length, 0);
      return { status: 0, stdout: JSON.stringify(baselineReport()) };
    }
    assert.equal(uid, UID);
    assert.equal(engine.state.puts.length, 1);
    return { status: 0, stdout: JSON.stringify(probeReport({ latencyFrames: { mean: REPORTED + 768 } })) };
  } });
  Object.assign(engine.options, { sourceUid: FOREIGN, sourceEndpoint: undefined });
  const summary = await runAcceptance(engine.options, engine.io);
  assert.equal(summary.reportedInputLatencyFrames, REPORTED);
  assert.equal(summary.sourceResidualFrames, 768);
  assert.equal(engine.read('source-baseline.json').latencyFrames.mean, 768);
  assert.deepEqual(engine.state.graph, { ...engine.original, revision: 5 });
});

test('a failed, incomplete or unrelated source baseline never mutates the graph', async t => {
  for (const [report, status] of [
    [baselineReport(), 3],
    [baselineReport({ measured: false }), 0],
    [baselineReport({ complete: false }), 0],
    [baselineReport({ input: { uid: 'other', rate: 48000 } }), 0],
    [baselineReport({ output: { uid: FOREIGN, rate: 44100 } }), 0],
    [baselineReport({ timestampDiscontinuities: { input: 1, output: 0 } }), 0],
    [baselineReport({ reportedLatencyFrames: null }), 0],
  ]) {
    const engine = fakeEngine(t, { roundtrip: () => ({ status, stdout: JSON.stringify(report) }) });
    Object.assign(engine.options, { sourceUid: FOREIGN, sourceEndpoint: undefined });
    await assert.rejects(runAcceptance(engine.options, engine.io));
    assert.deepEqual(engine.state.puts, []);
    assert.deepEqual(engine.state.graph, engine.original);
    assert.deepEqual(listeners(engine.signals), [0, 0, 0]);
  }
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  test(`${signal} during the measurement restores the confirmed graph, keeps evidence and re-raises`, async t => {
    let readings = 0;
    const engine = fakeEngine(t, {
      reported: (state, signals) => {
        if (state.graph.revision === 4 && ++readings === 2) signals.emit(signal, signal);
      },
    });
    await assert.rejects(runAcceptance(engine.options, engine.io), new RegExp(`Interrupted by ${signal}`));
    assert.deepEqual(engine.state.graph, { ...engine.original, revision: 5 });
    const failed = engine.read('failed.json');
    assert.equal(failed.interrupted, signal);
    assert.equal(failed.mutation, 'applied');
    assert.deepEqual(failed.errors, [`Error: Interrupted by ${signal}`]);
    assert.ok(existsSync(join(engine.evidence, 'restored.json')));
    assert.equal(existsSync(join(engine.evidence, 'measured.json')), false);
    assert.deepEqual(engine.signals.raised, [[777, signal]]);
    assert.deepEqual(listeners(engine.signals), [0, 0, 0]);
  });
}

test('an interruption waits for the test-graph write in flight, then restores it', async t => {
  const engine = fakeEngine(t, {
    put: (document, _, signals) => { if (document.revision === 4) signals.emit('SIGINT', 'SIGINT'); },
  });
  await assert.rejects(runAcceptance(engine.options, engine.io), /Interrupted by SIGINT/);
  assert.deepEqual(engine.state.puts.map(put => put.revision), [4, 5]);
  assert.deepEqual(engine.state.graph, { ...engine.original, revision: 5 });
  assert.equal(engine.read('failed.json').mutation, 'applied');
  assert.deepEqual(engine.signals.raised, [[777, 'SIGINT']]);
});

test('an interrupted write without a terminal result is never restored by guesswork', async t => {
  const engine = fakeEngine(t);
  const request = engine.io.request;
  engine.io.request = async (path, options) => {
    if (options.method === 'PUT' && JSON.parse(options.body).revision === 4) {
      engine.signals.emit('SIGTERM', 'SIGTERM');
      engine.signals.emit('SIGTERM', 'SIGTERM');
      throw new Error('reply lost');
    }
    return request(path, options);
  };
  await assert.rejects(runAcceptance(engine.options, engine.io), /did not finish/);
  assert.equal(engine.state.puts.some(put => put.revision === 5), false);
  const failed = engine.read('failed.json');
  assert.equal(failed.mutation, 'unknown');
  assert.equal(failed.interrupted, 'SIGTERM');
  assert.match(failed.errors[1], /no terminal result; check graph g1/);
  assert.match(engine.warnings.at(-1), /still restoring/);
  assert.deepEqual(engine.signals.raised, [[777, 'SIGTERM']]);
});

test('an interruption before any write changes nothing and re-raises', async t => {
  const engine = fakeEngine(t);
  const request = engine.io.request;
  engine.io.request = async (path, options) => {
    if (path === '/api/v1/status') engine.signals.emit('SIGHUP', 'SIGHUP');
    return request(path, options);
  };
  await assert.rejects(runAcceptance(engine.options, engine.io), /Interrupted by SIGHUP/);
  assert.deepEqual(engine.state.puts, []);
  assert.deepEqual(engine.signals.raised, [[777, 'SIGHUP']]);
});

test('a probe exit status 4 alone fails the run after restoring the graph', async t => {
  // The report itself passes; only the exit status says the recording was incomplete.
  const engine = fakeEngine(t, {
    roundtrip: () => ({ status: 4, stdout: JSON.stringify(probeReport()) }),
  });
  await assert.rejects(runAcceptance(engine.options, engine.io), /probe exited with status 4/);
  assert.deepEqual(engine.state.graph, { ...engine.original, revision: 5 });
  assert.equal(engine.read('failed.json').roundtrip.complete, true);
});

test('the round trip requires a measured, complete, continuous recording within tolerance', () => {
  assert.deepEqual(roundtripProblems(probeReport(), REPORTED), []);
  assert.equal(roundtripToleranceFrames(48000), 50);
  assert.equal(roundtripToleranceFrames(96000), 98);
  assert.equal(roundtripToleranceFrames(22050), 34);
  // The engine rounds 1 ms up to whole frames: 44.1 frames is 45, beyond 32.
  assert.equal(roundtripToleranceFrames(44100), 47);
  const within = probeReport({ latencyFrames: { mean: REPORTED + 50 } });
  assert.deepEqual(roundtripProblems(within, REPORTED), []);
  assert.deepEqual(roundtripProblems(probeReport({ latencyFrames: { mean: REPORTED + 768 + 50 } }), REPORTED, 768), []);
  assert.match(roundtripProblems(probeReport({ latencyFrames: { mean: REPORTED + 768 + 51 } }), REPORTED, 768).join(), /tolerance 50/);
  assert.match(roundtripProblems(probeReport(), REPORTED, Number.NaN).join(), /baseline is not finite/);
  for (const [report, problem] of [
    [probeReport({ measured: false }), /did not measure/],
    [probeReport({ complete: false }), /incomplete/],
    [probeReport({ timestampDiscontinuities: { output: 0, input: 1 } }), /timestamps jumped/],
    [probeReport({ timestampDiscontinuities: undefined }), /timestamps jumped/],
    [probeReport({ latencyFrames: { mean: REPORTED - 50.5 } }), /differs from the measured mean 429.5 by 50.50 frames \(tolerance 50\)/],
    [probeReport({ reportedLatencyFrames: null }), /do not report their full latency/],
    [probeReport({ latencyFrames: undefined }), /no measured latency/],
    [probeReport({ mode: undefined }), /impulse mode/],
    [probeReport({ output: { rate: 44100 } }), /one common rate/],
    [probeReport({ input: { rate: 48000, latency: 0 } }), /not the stable report/],
  ]) {
    assert.match(roundtripProblems(report, REPORTED).join('\n'), problem);
  }
});

test('the reported input latency is read from the probe text', () => {
  const text = 'output x\ninput  MixRelay (uid) \'virt\' 48000 Hz, 2 ch, buffer 512; reports latency 480 + safety 0 + stream 0 frames\n';
  assert.deepEqual(parseReported(text), { latency: 480, safety: 0, stream: 0 });
  assert.throws(() => parseReported('input unknown'), /Cannot read/);
});
