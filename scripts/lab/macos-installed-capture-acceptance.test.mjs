import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import test from 'node:test';
import { runCaptureAcceptance } from './macos-installed-capture-acceptance.mjs';

const ENDPOINT = '22222222-2222-4222-8222-222222222222';
function fixture(hook = () => {}) {
  const state = { graph: { revision: 3, nodes: [], edges: [] }, puts: 0, stopped: false, samples: 0 };
  const signals = new EventEmitter(); signals.kill = () => {};
  const io = {
    signals, warn() {}, async delay() {}, record(value) { state.evidence = value; },
    async startTone() { return { async stop() { state.stopped = true; } }; },
    async apply(document) {
      state.puts++;
      const { expectedGraphId, ...graph } = document; assert.equal(expectedGraphId, 'g');
      state.graph = structuredClone(graph);
      hook('apply', state);
      return { status: 200 };
    },
    async api(path) {
      hook(path, state);
      if (path === 'graphs') return { activeId: 'g', document: structuredClone(state.graph) };
      if (path === 'graph') return structuredClone(state.graph);
      if (path === 'graph/status') return { activeRevision: state.graph.revision };
      if (path === 'status') return { sampleFaults: {}, audioSession: { virtualEndpointSettings: { devices: [{ spec: { id: ENDPOINT, render: { channels: 2 }, capture: { channels: 2 } }, nativeDeviceIds: [`io.mixrelay.audio.endpoint.${ENDPOINT.replaceAll('-', '')}`] }] } } };
      if (path === 'meters') { state.samples++; return { revision: state.graph.revision, nodes: [{ nodeId: 'capture-acceptance-source', portId: 'audio', peaks: [0.05, 0.05] }] }; }
      return {};
    },
  };
  return { io, state };
}
test('a measured installed route restores its original graph and releases the tone', async () => {
  const { io, state } = fixture();
  await runCaptureAcceptance(ENDPOINT, io);
  assert.deepEqual(state.graph.nodes, []); assert.equal(state.puts, 2); assert.equal(state.stopped, true);
  assert.equal(state.evidence.passed, true);
});
test('missing PCM still restores and stops its owned source', async () => {
  const { io, state } = fixture(); const api = io.api;
  io.api = path => path === 'meters' ? { revision: state.graph.revision, nodes: [] } : api(path);
  await assert.rejects(runCaptureAcceptance(ENDPOINT, io), /Owned tone missing/);
  assert.deepEqual(state.graph.nodes, []); assert.equal(state.stopped, true);
});
test('a lost apply reply recovers only the confirmed owned revision', async () => {
  const { io, state } = fixture((path, state) => { if (path === 'apply' && state.puts === 1) throw new Error('lost reply'); });
  await assert.rejects(runCaptureAcceptance(ENDPOINT, io), /lost reply/);
  assert.deepEqual(state.graph.nodes, []); assert.equal(state.puts, 2); assert.equal(state.stopped, true);
});
test('a concurrent edit is preserved even when the acceptance fails', async () => {
  const { io, state } = fixture((path, state) => { if (path === 'meters') { state.graph.revision++; throw new Error('read failed'); } });
  await assert.rejects(runCaptureAcceptance(ENDPOINT, io), /Graph changed externally/);
  assert.equal(state.puts, 1); assert.equal(state.stopped, true); assert.ok(state.graph.nodes.length > 0);
});
test('an occupied graph is rejected before starting a source or submitting a graph', async () => {
  const { io, state } = fixture(); state.graph.nodes.push({ id: 'user' });
  io.startTone = () => assert.fail('must not start');
  await assert.rejects(runCaptureAcceptance(ENDPOINT, io), /empty selected graph/);
  assert.equal(state.puts, 0); assert.equal(state.graph.nodes[0].id, 'user');
});
test('an interrupted measurement restores before re-raising its signal', async () => {
  let raised;
  const { io, state } = fixture((path, state) => {
    if (path === 'meters' && state.samples === 5) io.signals.emit('SIGINT', 'SIGINT');
  });
  io.signals.kill = (_, signal) => { raised = signal; assert.deepEqual(state.graph.nodes, []); assert.equal(state.stopped, true); };
  await assert.rejects(runCaptureAcceptance(ENDPOINT, io), /Interrupted by SIGINT/);
  assert.equal(raised, 'SIGINT'); assert.equal(state.puts, 2);
});
test('the recorder diagnostic measures the actual recorder input and restores', async () => {
  const { io, state } = fixture();
  await runCaptureAcceptance(ENDPOINT, io, 'recorder');
  assert.equal(state.evidence.applied.nodes[1].type, 'recorder');
  assert.deepEqual(state.graph.nodes, []); assert.equal(state.stopped, true);
});
