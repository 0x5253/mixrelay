// Installed-HAL acceptance without service restarts, node replacement or edge
// deletion. Restore only our exact revision; never overwrite concurrent edits.
import { engineSocket, socketJson, socketRequest } from '../local-socket.mjs';
import { engineOperations } from './engine-operations.mjs';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const [inspect, endpointId, evidence, approval] = process.argv.slice(2);
assert.equal(process.platform, 'darwin');
assert.equal(approval, '--verify-installed-channels');
assert.ok(isAbsolute(inspect) && isAbsolute(evidence));
assert.match(endpointId, /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
assert.notEqual(endpointId, '00000000-0000-0000-0000-000000000000');
const uid = `io.mixrelay.audio.endpoint.${endpointId.replaceAll('-', '')}`;
const socket = engineSocket();
const { api: operationApi } = engineOperations({
  request: (path, options) => socketRequest(socket, path, options), now: Date.now, delay,
}, 'macos-endpoint-channel-probe');
async function api(path, body) {
  if (body !== undefined) return operationApi(path, body);
  return socketJson(socket, `/api/v1/${path}`, {
    headers: { host: 'mixrelay-engine' }, timeoutMs: 5000,
  });
}
function native() {
  return JSON.parse(execFileSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', '-'], {
    input: execFileSync(inspect, [uid], { timeout: 3000 }), encoding: 'utf8',
  }));
}
async function until(check) {
  for (let attempt = 0; attempt < 100; attempt++) {
    if (await check()) return;
    await delay(100);
  }
  assert.fail('HAL channel publication timeout');
}
const original = await api('graph');
const settings = (await api('status')).audioSession.virtualEndpointSettings;
const device = settings.devices.find(device => device.spec.id === endpointId);
assert.ok(device?.nativeDeviceIds.includes(uid), 'Endpoint is not published by the current engine');
const before = native();
const target = original.nodes.find(node => node.type === 'virtual_endpoint' && node.config.id === endpointId);
assert.ok(target?.config.render && target.config.capture);
// Refuse a geometry change that could invalidate even one existing cable.
for (const edge of original.edges) {
  if (edge.source.nodeId === target.id || edge.destination.nodeId === target.id) {
    assert.equal(edge.channelMapping.mode, 'explicit');
    for (const route of edge.channelMapping.routes) {
      if (edge.source.nodeId === target.id) assert.ok(route.sourceChannel < 7);
      if (edge.destination.nodeId === target.id) assert.ok(route.destinationChannel < 3);
    }
  }
}
mkdirSync(evidence, { mode: 0o700 });
writeFileSync(join(evidence, 'before.json'), JSON.stringify({ graph: original, native: before }, null, 2), { flag: 'wx' });
const test = structuredClone(original);
test.revision++;
const config = test.nodes.find(node => node.id === target.id).config;
config.render = { channels: 7, channelLayout: 'discrete' };
config.capture = { channels: 3, channelLayout: 'discrete' };
let changed = false;
try {
  assert.deepEqual(await api('graph'), original, 'Graph changed during preparation');
  await api('graph', test);
  changed = true;
  await until(async () => (await api('graph/status')).activeRevision === test.revision &&
    native().renderChannels === 7 && native().captureChannels === 3);
  const applied = await api('graph');
  assert.deepEqual(applied.nodes, test.nodes);
  assert.deepEqual(applied.edges, original.edges);
  writeFileSync(join(evidence, '7-inputs-3-outputs.json'), JSON.stringify({ graph: applied, native: native() }, null, 2), { flag: 'wx' });
} finally {
  if (changed) {
    assert.deepEqual(await api('graph'), test, 'Concurrent edit: refusing automatic restore; original saved in evidence');
    const restore = { ...original, revision: test.revision + 1 };
    await api('graph', restore);
    await until(async () => (await api('graph/status')).activeRevision === restore.revision &&
      JSON.stringify(native()) === JSON.stringify(before));
    writeFileSync(join(evidence, 'restored.json'), JSON.stringify({ graph: await api('graph'), native: native() }, null, 2), { flag: 'wx' });
  }
}
console.log('PASS installed HAL 7 inputs / 3 outputs, original nodes and cables preserved, original channel geometry restored');
