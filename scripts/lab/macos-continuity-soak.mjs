import { parseNativeTelemetry, continuityFailures, referenceFailures } from '../core/continuity-evidence.mjs';
// Read-only installed-engine acceptance. Retains failures and never resets counters.
import { engineSocket, socketJson } from '../local-socket.mjs';
import { open, readFile } from 'node:fs/promises';
import { isAbsolute } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import assert from 'node:assert/strict';

const [output, logPath, durationText = '600', referencePath] = process.argv.slice(2);
const duration = Number(durationText);
assert(output && logPath && isAbsolute(output) && isAbsolute(logPath), 'Use absolute output and engine log paths');
assert(Number.isInteger(duration) && duration >= 30 && duration <= 3600, 'Duration must be 30..3600 seconds');
assert(!referencePath || isAbsolute(referencePath), 'Reference output must use an absolute path');
const file = await open(output, 'wx');
const log = await open(logPath, 'r');
const buffer = Buffer.alloc(65536);
const failures = [];
const socket = engineSocket();
async function get(path) {
  return socketJson(socket, `/api/v1/${path}`, {
    headers: { host: 'mixrelay-engine' }, timeoutMs: 3000,
  });
}
async function trace() {
  const { size } = await log.stat();
  const { bytesRead } = await log.read(buffer, 0, Math.min(size, buffer.length), Math.max(0, size - buffer.length));
  return parseNativeTelemetry(buffer.toString('utf8', 0, bytesRead));
}
async function sample() {
  const [status, queues] = await Promise.all([get('status'), trace()]);
  return { at: new Date().toISOString(), status, ...queues };
}
async function emit(row) { await file.write(`${JSON.stringify(row)}\n`); }
let before;
try {
  const graphs = await Promise.all(['graph-1', 'graph-105'].map(id => get(`graphs/${id}`)));
  before = await sample();
  assert(Object.keys(before.devices).length > 0, 'No native device telemetry');
  await emit({ type: 'start', graphs, ...before });
  const started = Date.now();
  let last = before;
  while (Date.now() - started < duration * 1000) {
    await delay(Math.min(5000, duration * 1000 - (Date.now() - started)));
    last = await sample();
    await emit({ type: 'sample', ...last });
    failures.push(...continuityFailures(before, last, Date.now()));
    console.log(JSON.stringify({ elapsedSeconds: Math.round((Date.now() - started) / 1000), failures: failures.length,
      underruns: last.status.underruns - before.status.underruns, overruns: last.status.overruns - before.status.overruns }));
  }
  const afterGraphs = await Promise.all(['graph-1', 'graph-105'].map(id => get(`graphs/${id}`)));
  if (JSON.stringify(graphs) !== JSON.stringify(afterGraphs)) failures.push('Graph configuration/revision changed');
  // An individually quiet observation window must not erase errors accumulated
  // since the agreed pre-work baseline. Never reset native counters to make it pass.
  if (referencePath) {
    const reference = JSON.parse((await readFile(referencePath, 'utf8')).trim().split('\n').at(-1));
    failures.push(...referenceFailures(reference, last, afterGraphs));
  }
  await emit({ type: 'result', referencePath, durationSeconds: (Date.now() - started) / 1000, passed: failures.length === 0, failures,
    before, after: last, afterGraphs });
  process.exitCode = failures.length ? 1 : 0;
} catch (error) {
  await emit({ type: 'error', at: new Date().toISOString(), error: String(error) });
  throw error;
} finally {
  await log.close();
  await file.close();
}
