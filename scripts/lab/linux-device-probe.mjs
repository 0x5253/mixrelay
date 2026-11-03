// Native device boundary acceptance in an owned guest session. Fixtures stand
// in for multichannel hardware; they are not proof of a physical 64-channel ADC.
import { engineSocket, socketRequest } from '../local-socket.mjs';
import { engineOperations } from './engine-operations.mjs';
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync, readFileSync, openSync, closeSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { parsePwDump, nodesOwnedByPid } from './pipewire-cli.mjs';

assert.equal(process.platform, 'linux');
assert.notEqual(process.getuid(), 0);
const [enginePath, fixturePath, evidence] = process.argv.slice(2);
assert.ok([enginePath, fixturePath, evidence].every(isAbsolute));
const ipcDirectory = join(evidence, 'ipc');
const socket = engineSocket({ MIXRELAY_IPC_DIR: ipcDirectory });
mkdirSync(evidence, { mode: 0o700 });
let engine, fixture, run = 0;
const results = [];
function inventory() { return parsePwDump(execFileSync('pw-dump', { encoding: 'utf8', timeout: 5000 })); }
function ownNodes(pid) { return nodesOwnedByPid(inventory(), pid); }
async function until(check, message) {
  for (let n = 0; n < 120; n++) { if (await check()) return; await delay(50); }
  assert.fail(message);
}
const { operation } = engineOperations({
  request: (path, options) => socketRequest(socket, path, options),
  now: Date.now,
  delay,
}, 'device-probe');
async function api(path, body, rejected = false) {
  const response = await operation(body === undefined ? 'GET' : 'PUT', `/api/v1/${path}`,
    body === undefined ? undefined : JSON.stringify(body));
  assert.equal(response.status >= 200 && response.status < 300, !rejected,
    `${path}: HTTP ${response.status} ${response.body}`);
  return response.json;
}
function launch(path, args, log) {
  const fd = openSync(join(evidence, log), 'wx', 0o600);
  const child = spawn(path, args, { env: { ...process.env, MIXRELAY_CONFIG_DIR: join(evidence, 'config'), MIXRELAY_IPC_DIR: ipcDirectory }, stdio: ['ignore', fd, fd] });
  closeSync(fd);
  child.on('error', error => { child.launchError = error; });
  return child;
}
async function stop(child) {
  if (!child) return;
  assert.equal(child.launchError, undefined);
  if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
  await until(() => child.exitCode !== null || child.signalCode !== null, 'Owned process failed to stop');
  assert.equal(child.exitCode, 0, 'Owned process did not shut down cleanly');
  await until(() => ownNodes(child.pid).length === 0, 'Native nodes leaked after process exit');
}
async function apply(nodes, edges = [], rejected = false) {
  const graph = await api('graph');
  const revision = graph.revision + 1;
  const result = await api('graph', { ...graph, revision, nodes, edges }, rejected);
  if (rejected) { assert.deepEqual(await api('graph'), graph); return result; }
  await until(async () => (await api('graph/status')).activeRevision === revision, 'Graph did not activate');
}
function devices(input, output, gain = 0) {
  return [
    { id: 'device-input', type: 'audio_device_input', config: { selector: { match: 'persistent_uid', uid: 'io.mixrelay.lab-device-source' }, outputChannels: input, gainDb: gain } },
    { id: 'device-output', type: 'audio_device_output', config: { selector: { match: 'persistent_uid', uid: 'io.mixrelay.lab-device-sink' }, inputChannels: output } },
  ];
}
function edges(routes) {
  return routes.map(([sourceChannel, destinationChannel], n) => ({ id: `cable-${n}`,
    source: { nodeId: 'device-input', portId: 'audio' }, destination: { nodeId: 'device-output', portId: 'audio' },
    channelMapping: { mode: 'explicit', routes: [{ sourceChannel, destinationChannel }] } }));
}
function nativeIds() {
  return ownNodes(engine.pid).filter(n => /\.device-(input|output)$/.test(n.info.props['node.name']))
    .map(n => n.info.props['object.serial']).sort();
}
async function startFixture(input, output) {
  fixture = launch(fixturePath, [String(input), String(output)], `fixture-${++run}.json`);
  await until(() => ownNodes(fixture.pid).length === 2, 'Fixture source/sink failed to register');
}
async function signalTest(input, output, routes, gainEdit = false) {
  await apply([]);
  await until(() => nativeIds().length === 0, 'Retired device streams leaked');
  await startFixture(input, output);
  const nodes = devices(input, output);
  await apply(nodes, edges(routes));
  const initialIds = nativeIds();
  assert.equal(initialIds.length, 2);
  if (gainEdit) {
    await delay(200);
    nodes[0].config.gainDb = -6.0206;
    await apply(nodes, edges(routes));
    assert.deepEqual(nativeIds(), initialIds, 'Gain edit reopened native I/O');
    await delay(400);
    fixture.kill('SIGUSR1');
  }
  await delay(1700);
  const meters = await api('meters');
  writeFileSync(join(evidence, `meters-${run}.json`), JSON.stringify(meters, null, 2), { flag: 'wx' });
  await stop(fixture); fixture = undefined;
  const measured = JSON.parse(readFileSync(join(evidence, `fixture-${run}.json`), 'utf8'));
  assert.equal(measured.invalid, 0);
  assert.ok(measured.maximumGap < 48, `Internal silent gap: ${measured.maximumGap} frames`);
  assert.ok(measured.active > 24000, `No sustained ${input} -> ${output} PCM: ${JSON.stringify(measured)}`);
  const expected = Array(output).fill(0);
  for (const [s, d] of routes) expected[d] += (s + 1) / 1024 * (gainEdit ? 0.5 : 1);
  const inputMeter = meters.nodes.find(n => n.nodeId === 'device-input');
  const outputMeter = meters.nodes.find(n => n.nodeId === 'device-output');
  assert.equal(inputMeter?.peaks.length, input); assert.equal(outputMeter?.peaks.length, output);
  for (let c = 0; c < input; c++) assert.ok(Math.abs(inputMeter.peaks[c] - (c + 1) / 1024 * (gainEdit ? 0.5 : 1)) < 0.00002, `Input meter ${c + 1}`);
  for (let c = 0; c < output; c++) assert.ok(Math.abs(outputMeter.peaks[c] - expected[c]) < 0.00002, `Output meter ${c + 1}`);
  for (let c = 0; c < output; c++) assert.ok(Math.abs(measured.peaks[c] - expected[c]) < 0.00002,
    `Channel ${c + 1}: expected ${expected[c]}, got ${measured.peaks[c]}`);
  await delay(150);
  assert.equal((await api('status')).audioSession.processingState, 'running', 'Device loss stopped graph clock');
  await until(async () => (await api('meters')).nodes.every(n => n.peaks.every(p => p === 0)),
    'Disconnected device replayed stale meter data');
  results.push({ input, output, routes, gainEdit, ...measured });
  console.log(`PASS native device ${input} -> ${output}, gainEdit=${gainEdit}`);
}

async function recoveryTest() {
  await apply([]); await startFixture(64, 16);
  const nodes = devices(64, 16, -6.0206);
  await apply(nodes, edges([[12, 0], [32, 15]]));
  let expectedGraph = await api('graph');
  const events = [];
  const serial = direction => {
    const streams = ownNodes(engine.pid).filter(n => n.info.props['node.name']?.endsWith(`.device-${direction}`));
    assert.ok(streams.length <= 1, `Multiple owned ${direction} streams`);
    return streams[0]?.info.props['object.serial'];
  };
  const meter = async id => (await api('meters')).nodes.find(n => n.nodeId === id)?.peaks;
  async function toggleSide(source, present) {
    fixture.kill(source ? 'SIGUSR2' : 'SIGHUP');
    await until(() => ownNodes(fixture.pid).some(n => n.info.props['node.name'] === `io.mixrelay.lab-device-${source ? 'source' : 'sink'}`) === present, 'Fixture presence did not change');
  }
  async function audible() {
    await until(async () => {
      const peaks = await meter('device-output');
      return peaks?.length === 16 && Math.abs(peaks[0] - 13 / 2048) < 0.00002 && Math.abs(peaks[15] - 33 / 2048) < 0.00002;
    }, 'Same-graph device recovery did not restore PCM/meter levels');
    assert.deepEqual(await api('graph'), expectedGraph, 'Recovery changed graph/history revision');
  }
  await audible();
  for (let cycle = 0; cycle < 4; cycle++) {
    const heldOutput = serial('output');
    const previousInput = serial('input');
    await toggleSide(true, false);
    await until(async () => (await meter('device-input')).every(value => value === 0), 'Lost input not silent');
    await delay(650); // Exercise a failed retry while the named device is absent.
    const started = performance.now();
    await toggleSide(true, true); await audible();
    const elapsedMs = performance.now() - started;
    assert.notEqual(serial('input'), previousInput);
    assert.equal(serial('output'), heldOutput, 'Input recovery reopened healthy output');
    events.push({ cycle, elapsedMs, heldOutput });
  }
  const heldInput = serial('input');
  await toggleSide(false, false); await delay(650);
  assert.ok((await meter('device-input'))[32] > 0.01, 'Lost output stopped healthy input');
  await toggleSide(false, true); await audible();
  assert.equal(serial('input'), heldInput, 'Output recovery reopened healthy input');

  await toggleSide(true, false); await toggleSide(false, false); await delay(650);
  await until(async () => (await meter('device-input')).every(value => value === 0), 'Both-lost input not silent');
  await toggleSide(true, true);
  await until(async () => serial('input') !== undefined && (await meter('device-input'))[32] > 0.01, 'Input could not recover while output absent');
  const independentInput = serial('input');
  await toggleSide(false, true); await audible();
  assert.equal(serial('input'), independentInput);

  // A gain edit while offline must remain editable and be used on recovery.
  await toggleSide(true, false); await delay(100);
  nodes[0].config.gainDb = -12.0412;
  await apply(nodes, edges([[12, 0], [32, 15]])); expectedGraph = await api('graph');
  await toggleSide(true, true);
  await until(async () => Math.abs((await meter('device-output'))[15] - 33 / 4096) < 0.00002, 'Offline gain edit lost');
  assert.deepEqual(await api('graph'), expectedGraph);

  // Independently record the post-recovery, edited PCM, not only engine meters.
  await delay(400); fixture.kill('SIGUSR1'); await delay(1000);

  // Reconnection must not resurrect nodes deleted while their device is absent.
  await toggleSide(true, false); await delay(650); await apply([]);
  const empty = await api('graph');
  await toggleSide(true, true); await delay(1200);
  assert.equal(nativeIds().length, 0, 'Deleted nodes resurrected on device arrival');
  assert.deepEqual(await api('graph'), empty);
  await stop(fixture); fixture = undefined;
  const pcm = JSON.parse(readFileSync(join(evidence, `fixture-${run}.json`), 'utf8'));
  assert.equal(pcm.invalid, 0); assert.ok(pcm.active > 24000, 'No sustained PCM after recovery');
  for (let channel = 0; channel < 16; channel++) {
    const expected = channel === 0 ? 13 / 4096 : channel === 15 ? 33 / 4096 : 0;
    assert.ok(Math.abs(pcm.peaks[channel] - expected) < 0.00002, `Recovered PCM channel ${channel + 1}`);
  }
  writeFileSync(join(evidence, 'recovery.json'), JSON.stringify({ events, pcm, independentArrival: true, healthyStreamsRetained: true, offlineGainEdit: true, deletedNodesStayDeleted: true }, null, 2), { flag: 'wx' });
  console.log('PASS same-graph recovery, healthy stream retention, independent arrival, offline gain and deletion');
}
try {
  engine = launch(enginePath, [], 'engine.log');
  await until(async () => { try { await api('status'); return true; } catch { return false; } }, 'Engine did not start');
  const caps = (await api('status')).audioSession.capabilities;
  assert.equal(caps.hardwareInput, true); assert.equal(caps.hardwareOutput, true);
  for (const channels of [2, 8, 16, 32, 64])
    await signalTest(channels, channels, Array.from({ length: channels }, (_, c) => [c, channels - c - 1]));
  await signalTest(64, 16, [[12, 0], [32, 15]], true);
  await signalTest(8, 2, [[0, 0], [2, 0], [1, 1], [3, 1]]);
  await recoveryTest();
  await apply([]); await startFixture(64, 16);
  for (const mutate of [
    nodes => { nodes[0].config.selector.uid = 'missing-device'; },
    nodes => { nodes[0].config.selector = { match: 'default_output' }; },
    nodes => { nodes[1].config.inputChannels = 32; },
  ]) {
    const nodes = devices(64, 16); mutate(nodes); await apply(nodes, [], true);
    assert.equal(nativeIds().length, 0, 'Rejected apply left partial streams');
  }
  const rss = [];
  for (let n = 0; n < 32; n++) {
    await apply(devices(64, 16)); await apply([]);
    await until(() => nativeIds().length === 0, 'Device stream leaked on graph removal');
    await delay(550); // Include the recovery worker's deferred-retirement interval.
    rss.push(Number(readFileSync(`/proc/${engine.pid}/status`, 'utf8').match(/^VmRSS:\s+(\d+)/m)?.[1]));
  }
  writeFileSync(join(evidence, 'teardown-rss.json'), JSON.stringify(rss), { flag: 'wx' });
  console.log(JSON.stringify({ teardownRssKiB: rss }));
  assert.ok(Math.max(...rss.slice(4)) - Math.min(...rss.slice(4)) < 8192, 'Unbounded short-run resource growth');
  await stop(fixture); fixture = undefined;
  await stop(engine); engine = undefined;
  writeFileSync(join(evidence, 'result.json'), JSON.stringify({ result: 'passed', results, teardownRssKiB: rss,
    notVerified: ['physical 64-channel hardware', 'long endurance', 'audio server restart', 'other OS'] }, null, 2), { flag: 'wx' });
  console.log('PASS device direction/identity rejection, 32 teardown cycles, owned process cleanup');
} catch (error) {
  writeFileSync(join(evidence, 'failed-inventory.json'), JSON.stringify(inventory(), null, 2), { flag: 'wx' });
  console.error(error);
  throw error;
} finally {
  // Always stop both owners even when one shutdown assertion fails.
  const cleanup = await Promise.allSettled([stop(fixture), stop(engine)]);
  for (const result of cleanup) if (result.status === 'rejected') console.error('Cleanup:', result.reason);
}
