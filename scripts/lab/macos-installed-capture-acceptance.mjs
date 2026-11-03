import { invoke } from '../runtime-control.mjs';
// Installed desktop tap -> SRC -> application IPC -> graph PCM acceptance.
// Uses only the explicit owned virtual-tone fixture and an initially empty graph.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdirSync, writeFileSync } from 'node:fs';
import { isAbsolute, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { engineSocket, socketRequest } from '../local-socket.mjs';
import { engineOperations } from './engine-operations.mjs';
import { watchInterruptions } from './macos-latency-acceptance.mjs';

const BUNDLE = 'io.mixrelay.test-virtual-clock-source';
const SOURCE = 'capture-acceptance-source';
const DESTINATION = 'capture-acceptance-destination';
const delay = ms => new Promise(resolveDelay => setTimeout(resolveDelay, ms));
const USAGE = 'node scripts/lab/macos-installed-capture-acceptance.mjs TONE_EXECUTABLE ENDPOINT_UUID FRESH_EVIDENCE_DIR --verify-installed-capture [--sink=recorder]';

function testGraph(original, graphId, spec, sink) {
  return { ...original, revision: original.revision + 1, expectedGraphId: graphId,
    nodes: [
      { id: SOURCE, label: 'Owned capture proof', type: 'application_capture', config: {
        selector: { match: 'bundle_id', bundle_id: BUNDLE, include_descendants: false },
        stream: { channels: 2, channelLayout: 'stereo' }, sourcePlayback: 'preserve',
      } },
      sink === 'recorder'
        ? { id: DESTINATION, label: 'Owned capture recorder', type: 'recorder', config: { channels: 2 } }
        : { id: DESTINATION, label: 'Owned capture output', type: 'virtual_endpoint',
          config: { ...spec, reference: { render: false, capture: true } } },
    ], edges: [{ id: 'capture-acceptance-edge', source: { nodeId: SOURCE, portId: 'audio' },
      destination: { nodeId: DESTINATION, portId: sink === 'recorder' ? 'audio' : 'to_os' }, channelMapping: { mode: 'auto' } }],
  };
}

function ownsGraph(current, expected) {
  const source = current.nodes.find(node => node.id === SOURCE);
  const destination = current.nodes.find(node => node.id === DESTINATION);
  return current.revision === expected.revision && current.nodes.length === 2
    && source?.type === 'application_capture' && source.config.selector.bundle_id === BUNDLE
    && source.config.sourcePlayback === 'preserve'
    && destination?.type === expected.nodes[1].type
    && (destination.type === 'recorder' ? destination.config.channels === 2
      : destination.config.id === expected.nodes[1].config.id)
    && JSON.stringify(current.edges) === JSON.stringify(expected.edges);
}

async function waitActive(io, revision, interrupt) {
  for (let attempt = 0; attempt < 100; attempt++) {
    interrupt?.check();
    if ((await io.api('graph/status')).activeRevision === revision) return;
    await io.delay(100);
  }
  assert.fail(`Graph ${revision} did not become active`);
}

/** All OS, process, control, time, signal and evidence boundaries are injected. */
export async function runCaptureAcceptance(endpointId, io, sink = 'virtual_endpoint') {
  const interrupt = watchInterruptions(io.signals, io.warn);
  const evidence = { meters: [] };
  let tone;
  let document;
  let original;
  let graphId;
  let submitted = false;
  try {
    const catalog = await io.api('graphs');
    original = catalog.document;
    graphId = catalog.activeId;
    assert.equal(original.nodes.length, 0, 'Requires an empty selected graph');
    assert.equal(original.edges.length, 0, 'Requires an empty selected graph');
    const status = await io.api('status');
    const endpoint = status.audioSession.virtualEndpointSettings.devices.find(device => device.spec.id === endpointId);
    assert.ok(endpoint && endpoint.spec.render.channels === 2 && endpoint.spec.capture.channels === 2,
      'Requires an existing stereo owned endpoint');
    const uid = `io.mixrelay.audio.endpoint.${endpointId.replaceAll('-', '')}`;
    assert.ok(endpoint.nativeDeviceIds.includes(uid), 'Exact native endpoint identity required');
    document = testGraph(original, graphId, endpoint.spec, sink);
    evidence.original = original;
    evidence.graphId = graphId;
    interrupt.check();
    tone = await io.startTone(uid);
    interrupt.check();
    submitted = true;
    evidence.apply = await io.apply(document);
    await waitActive(io, document.revision, interrupt);
    interrupt.check();
    evidence.applied = await io.api('graph');
    assert.ok(ownsGraph(evidence.applied, document), 'Unexpected applied graph');
    // Three seconds covers startup and ten successive final meter windows.
    for (let index = 0; index < 30; index++) {
      interrupt.check();
      await io.delay(100);
      evidence.meters.push(await io.api('meters'));
    }
    evidence.status = await io.api('status');
    evidence.manifest = await io.api('application-capture/manifest');
    assert.ok(evidence.meters.slice(-10).every(meter => meter.revision === document.revision),
      'Meter windows belong to a different graph revision');
    const healthy = evidence.meters.slice(-10).map(meter =>
      meter.nodes.find(node => node.nodeId === SOURCE && node.portId === 'audio'));
    assert.ok(healthy.every(node => node?.peaks.length === 2
      && node.peaks.every(peak => Number.isFinite(peak) && peak > 0.04 && peak < 0.06)),
    'Owned tone missing from installed capture/SRC/IPC/graph');
    interrupt.check();
    evidence.passed = true;
    return evidence;
  } catch (error) {
    evidence.error = error.message;
    throw error;
  } finally {
    try {
      if (submitted) {
        const catalog = await io.api('graphs');
        const current = catalog.document;
        assert.equal(catalog.activeId, graphId, 'Selection changed externally; preserve it');
        if (JSON.stringify(current) !== JSON.stringify(original)) {
          assert.ok(ownsGraph(current, document), 'Graph changed externally; preserve it');
          if (evidence.applied) assert.deepEqual(current, evidence.applied, 'Graph changed externally; preserve it');
          evidence.restore = await io.apply({ ...original, revision: current.revision + 1, expectedGraphId: graphId });
          await waitActive(io, current.revision + 1);
          evidence.restored = await io.api('graph');
          assert.deepEqual(evidence.restored.nodes, original.nodes);
          assert.deepEqual(evidence.restored.edges, original.edges);
        }
      }
    } finally {
      try { await tone?.stop(); } finally {
        try { io.record(evidence); } finally {
          interrupt.dispose();
          if (interrupt.received) io.signals.kill(io.signals.pid, interrupt.received);
        }
      }
    }
  }
}

async function startTone(executable, uid) {
  const child = spawn(executable, ['--mixrelay-virtual-only', uid], { stdio: ['pipe', 'pipe', 'inherit'] });
  child.stdin.on('error', () => {}); // A child that already closed stdin still needs reaping.
  const failed = new Promise((_, reject) => child.once('error', reject));
  const stop = async () => {
    if (!child.pid || child.exitCode !== null || child.signalCode !== null) return;
    child.stdin.end('quit\n');
    await Promise.race([once(child, 'exit'), delay(2000)]);
    if (child.exitCode === null && child.signalCode === null) { child.kill('SIGKILL'); await once(child, 'exit'); }
  };
  try {
    const [ready] = await Promise.race([once(child.stdout, 'data'), failed,
      delay(5000).then(() => { throw new Error('Tone startup timed out'); })]);
    assert.equal(ready.toString().trim(), `ready ${uid}`);
    return { stop };
  } catch (error) { await stop(); throw error; }
}

const isCli = process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href;
if (isCli && process.argv[2] === '--help') {
  console.log(`${USAGE}\nRequires the compiled MRVirtualToneSource fixture and an empty selected graph.\nMeasures installed tap/SRC/IPC/graph PCM with no physical/default output. Restores its owned revision and stops its child on failure or interruption.\n--sink=recorder measures the graph recorder input without opening a recording file.\nExisting graphs and changed selections are preserved. Evidence is written to a fresh private directory.`);
} else if (isCli) {
  const [tone, endpoint, evidence, flag, sinkFlag] = process.argv.slice(2);
  try {
    assert.equal(process.platform, 'darwin');
    assert.ok(tone && isAbsolute(tone) && evidence && isAbsolute(evidence), 'Use absolute tone and fresh evidence paths');
    assert.match(endpoint ?? '', /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
    assert.equal(flag, '--verify-installed-capture', USAGE);
    assert.ok(sinkFlag === undefined || sinkFlag === '--sink=recorder');
    mkdirSync(evidence, { mode: 0o700 });
    const { api } = engineOperations({ request: (path, options) => socketRequest(engineSocket(), path, options), now: Date.now, delay }, 'capture-acceptance');
    const result = await runCaptureAcceptance(endpoint, { api, delay, signals: process, warn: console.error,
      startTone: uid => startTone(tone, uid),
      async apply(document) { const reply = await invoke('control_apply_graph', { document }); assert.ok(reply.status >= 200 && reply.status < 300, JSON.stringify(reply)); return reply; },
      record: result => writeFileSync(join(evidence, 'capture.json'), JSON.stringify(result, null, 2), { flag: 'wx' }),
    }, sinkFlag ? 'recorder' : 'virtual_endpoint');
    console.log(JSON.stringify({ passed: result.passed, lastMeters: result.meters.at(-1), sampleFaults: result.status.sampleFaults, restored: result.restored }));
  } catch (error) { console.error(error); process.exitCode = 1; }
}
