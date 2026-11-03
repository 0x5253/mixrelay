// Isolated user-session restart qualification. Never run over a user's engine.
import { engineSocket, socketJson, socketRequest } from '../local-socket.mjs';
import { engineOperations } from './engine-operations.mjs';
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdirSync, openSync, closeSync, readFileSync, writeFileSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { parsePwDump, nodesOwnedByPid } from './pipewire-cli.mjs';

assert.equal(process.platform, 'linux');
assert.notEqual(process.getuid(), 0);
const [engine, evidence] = process.argv.slice(2);
assert.ok(isAbsolute(engine) && isAbsolute(evidence));
const ipcDirectory = join(evidence, 'ipc');
const socket = engineSocket({ MIXRELAY_IPC_DIR: ipcDirectory });
mkdirSync(evidence, { mode: 0o700 }); // Exclusive, never overwrite earlier evidence.
const config = join(evidence, 'config');
const uid = process.getuid();
let child;
let run = 0;
const { api: operationApi } = engineOperations({
  request: (path, options) => socketRequest(socket, path, options), now: Date.now, delay,
}, 'endpoint-restart-probe');
async function api(path, body) {
  if (body !== undefined) return operationApi(path, body);
  return socketJson(socket, `/api/v1/${path}`, {
    headers: { host: 'mixrelay-engine' }, timeoutMs: 3000,
  });
}
async function until(check, message) {
  for (let n = 0; n < 150; n++) {
    if (await check()) return;
    await delay(40);
  }
  assert.fail(message);
}
function ownNodes(pid) {
  return nodesOwnedByPid(parsePwDump(execFileSync('pw-dump', { encoding: 'utf8', timeout: 5000 })), pid);
}
async function start() {
  const fd = openSync(join(evidence, `engine-${++run}.log`), 'wx', 0o600);
  try {
    child = spawn(engine, [], { stdio: ['ignore', fd, fd], env: { ...process.env,
      MIXRELAY_CONFIG_DIR: config, MIXRELAY_IPC_DIR: ipcDirectory, XDG_RUNTIME_DIR: `/run/user/${uid}`,
      DBUS_SESSION_BUS_ADDRESS: `unix:path=/run/user/${uid}/bus` } });
  } finally { closeSync(fd); }
  child.on('error', error => { console.error(error); });
  await until(async () => {
    assert.equal(child.exitCode, null, 'Engine exited during startup');
    try { await api('status'); return true; } catch { return false; }
  }, 'Engine API did not start');
  const graph = await api('graph');
  await until(async () => (await api('graph/status')).activeRevision === graph.revision, 'Restore did not activate');
  return graph;
}
async function stop() {
  if (!child) return;
  const owned = child;
  owned.kill('SIGTERM');
  await until(() => owned.exitCode !== null || owned.signalCode !== null, 'Owned engine did not exit after SIGTERM');
  assert.equal(owned.exitCode, 0, 'Engine shutdown was not clean');
  await until(() => ownNodes(owned.pid).length === 0, 'Engine-owned PipeWire nodes leaked');
  child = undefined;
}
async function apply(nodes) {
  const graph = await api('graph');
  const revision = graph.revision + 1;
  await api('graph', { ...graph, revision, nodes, edges: [] });
  await until(async () => (await api('graph/status')).activeRevision === revision, 'Edit did not activate');
  assert.equal((await api('status')).audioSession.endpointPreferencesUnavailable, false);
}
async function checkSides(expected) {
  writeFileSync(join(evidence, `inventory-${run}-${expected}.json`), execFileSync('pw-dump', { encoding: 'utf8', timeout: 5000 }), { flag: 'wx' });
  await until(() => ownNodes(child.pid).filter(n => n.info.props['node.name'].startsWith('io.mixrelay.restart-probe')).length === expected, 'Native device sides not restored');
  writeFileSync(join(evidence, `nodes-${run}.json`), JSON.stringify(ownNodes(child.pid), null, 2), { flag: 'wx' });
}

try {
  await start();
  const node = { id: 'restart-probe', type: 'virtual_endpoint', label: 'Restart 64 to 16', config: {
    id: 'restart-probe', displayName: 'Restart 64 to 16',
    render: { channels: 64, channelLayout: 'discrete' },
    capture: { channels: 16, channelLayout: 'discrete' },
    supportedSampleRates: [48000], defaultSampleRate: 48000 } };
  await apply([node]); await checkSides(2); await stop();
  const restored = await start();
  assert.deepEqual(restored.nodes[0].config, node.config);
  assert.deepEqual(restored.edges, []); await checkSides(2);
  await apply([{ ...node, config: { ...node.config, capture: null } }]); await stop();
  const half = await start();
  assert.equal(half.nodes[0].config.capture, null); await checkSides(1);
  await apply([]); await stop();
  const empty = await start();
  assert.deepEqual(empty.nodes, []); await checkSides(0);
  const status = await api('status');
  const template = status.audioSession.virtualEndpointSettings.defaultEndpoint;
  assert.deepEqual(template.spec, node.config);
  await apply([{ id: template.nodeId, type: 'virtual_endpoint', config: template.spec }]);
  await stop();
  console.log('PASS: four processes, 64/16 restore, half-side removal, full removal, template re-add, and native cleanup');
  console.log(readFileSync(join(config, 'virtual-endpoint.json'), 'utf8'));
} finally {
  await stop();
}
