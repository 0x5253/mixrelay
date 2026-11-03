// Explicit installed-service acceptance. Saves the live graph before any restart
// and restores it in finally; never restarts coreaudiod or changes default devices.
import { engineSocket, socketJson, socketRequest } from '../local-socket.mjs';
import { engineOperations } from './engine-operations.mjs';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

assert.equal(process.platform, 'darwin');
const [engine, inspect, evidence, approval] = process.argv.slice(2);
assert.equal(approval, '--restart-installed-service');
assert.ok([engine, inspect, evidence].every(isAbsolute));
const target = 'system/io.mixrelay.audio.engine';
const endpointId = randomUUID();
const uid = `io.mixrelay.audio.endpoint.${endpointId.replaceAll('-', '')}`;
const nodeId = `macos-restart-probe-${endpointId}`;
let restarts = 0;
let ownedRevision;
let ownedGraph;
function servicePid() {
  const state = execFileSync('/bin/launchctl', ['print', target], { encoding: 'utf8' });
  assert.ok(state.includes(`program = ${engine}\n`), 'Unexpected installed engine executable');
  assert.doesNotMatch(state, /MIXRELAY_CONFIG_DIR\s*=>/, 'Nonstandard service config; refusing live test');
  const pid = Number(state.match(/\n\s*pid = (\d+)/)?.[1]);
  assert.ok(Number.isSafeInteger(pid) && pid > 1);
  const owner = execFileSync('/bin/ps', ['-p', String(pid), '-o', 'uid='], { encoding: 'utf8' }).trim();
  assert.equal(Number(owner), process.getuid(), 'Refusing to signal another user');
  return pid;
}
const socket = engineSocket();
const { api: operationApi } = engineOperations({
  request: (path, options) => socketRequest(socket, path, options), now: Date.now, delay,
}, 'macos-endpoint-restart-probe');
async function api(path, body) {
  if (body !== undefined) return operationApi(path, body);
  return socketJson(socket, `/api/v1/${path}`, {
    headers: { host: 'mixrelay-engine' }, timeoutMs: 3000,
  });
}
async function until(check, message) {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    if (await check()) return;
    await delay(100);
  }
  assert.fail(message);
}
async function restart() {
  const before = await api('graph');
  assert.equal(before.revision, ownedRevision, 'Concurrent edit: refusing restart');
  assert.deepEqual(before.nodes, ownedGraph.nodes);
  assert.deepEqual(before.edges, ownedGraph.edges);
  const pid = servicePid();
  process.kill(pid, 'SIGTERM'); // launchd owns respawn; no global/process-name kill.
  await until(async () => {
    try { return servicePid() !== pid && !!await api('status'); } catch { return false; }
  }, 'Installed service did not restart');
  restarts++;
  const graph = await api('graph');
  await until(async () => (await api('graph/status')).activeRevision === graph.revision, 'Restored graph inactive');
  assert.deepEqual(graph.nodes, ownedGraph.nodes);
  assert.deepEqual(graph.edges, ownedGraph.edges);
  ownedRevision = graph.revision;
  return graph;
}
async function apply(nodes, edges) {
  const graph = await api('graph');
  assert.equal(graph.revision, ownedRevision, 'Concurrent edit: refusing to overwrite graph');
  const revision = graph.revision + 1;
  await api('graph', { ...graph, revision, nodes, edges });
  ownedRevision = revision;
  ownedGraph = { ...graph, revision, nodes, edges };
  await until(async () => (await api('graph/status')).activeRevision === revision, 'Graph did not activate');
  assert.equal((await api('status')).audioSession.endpointPreferencesUnavailable, false);
}
function native() {
  const plist = execFileSync(inspect, [uid], { timeout: 3000 });
  return JSON.parse(execFileSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', '-'], { input: plist, encoding: 'utf8' }));
}
async function checkNative(name, render, capture, label) {
  try { await until(() => {
    const result = native();
    return result.displayName === name && result.renderChannels === render && result.captureChannels === capture;
  }, `HAL did not publish ${name} ${render}/${capture}`); }
  finally {
    writeFileSync(join(evidence, `${label}.json`), JSON.stringify({ native: native(), graph: await api('graph'), status: await api('status') }, null, 2), { flag: 'wx' });
  }
}
servicePid();
const original = await api('graph');
ownedRevision = original.revision;
ownedGraph = original;
const settings = (await api('status')).audioSession.virtualEndpointSettings;
assert.ok(settings.canCreate && settings.creationTemplate, 'Endpoint creation is unavailable');
assert.ok(settings.renderChannels.includes(64) && settings.captureChannels.includes(16));
// A plugin restart would require instance-state persistence outside this probe.
assert.ok(original.nodes.every(n => n.type === 'virtual_endpoint'), 'Live graph contains other resources; refusing destructive restart');
mkdirSync(evidence, { mode: 0o700 });
writeFileSync(join(evidence, 'original-graph.json'), JSON.stringify(original, null, 2), { flag: 'wx' });
const node = { id: nodeId, type: 'virtual_endpoint', label: 'MixRelay 복원 64·16',
  control: { enabled: true, muted: false }, config: {
  ...settings.creationTemplate, id: endpointId, displayName: 'MixRelay 복원 64·16',
  render: { channels: 64, channelLayout: 'discrete' }, capture: { channels: 16, channelLayout: 'discrete' },
  reference: { render: true, capture: true } } };
const withProbe = probe => [...original.nodes, probe];
const published = async () => (await api('status')).audioSession.virtualEndpointSettings.devices
  .find(device => device.spec.id === endpointId);
try {
  await restart(); // Load the newly built binary before exercising its contract.
  await apply(withProbe(node), original.edges); await checkNative(node.label, 64, 16, 'applied');
  assert.ok((await published()).nativeDeviceIds.includes(uid));
  const restored = await restart();
  assert.deepEqual(restored.nodes.find(n => n.id === nodeId).config, node.config);
  assert.deepEqual(restored.edges, original.edges);
  await checkNative(node.label, 64, 16, 'restored');
  await apply(withProbe({ ...node, config: { ...node.config, reference: { render: true, capture: false } } }), original.edges);
  const half = await restart();
  assert.equal(half.nodes.find(n => n.id === nodeId).config.reference.capture, false);
  assert.equal((await published()).spec.capture.channels, 16);
  await checkNative(node.label, 64, 16, 'half-reference');
  await apply(original.nodes, original.edges);
  assert.deepEqual((await restart()).nodes, original.nodes);
  assert.equal(await published(), undefined);
  await until(() => {
    try { native(); return false; } catch (error) { return error.status === 2; }
  }, 'HAL retained the removed probe endpoint');
  await apply(withProbe(node), original.edges);
  await checkNative(node.label, 64, 16, 'readded');
  console.log(`PASS: ${restarts} installed-service restarts; native name/64/16 restore, half/full deletion and recreation`);
} finally {
  await apply(original.nodes, original.edges);
  const restored = await api('graph');
  assert.deepEqual(restored.nodes, original.nodes);
  assert.deepEqual(restored.edges, original.edges);
  assert.equal(await published(), undefined);
  await until(() => {
    try { native(); return false; } catch (error) { return error.status === 2; }
  }, 'HAL retained the probe endpoint after cleanup');
  console.log('Original nodes and cables restored; probe endpoint removed. Graph activation is not PCM continuity proof.');
}
