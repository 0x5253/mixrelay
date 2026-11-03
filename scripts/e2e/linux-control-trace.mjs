import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { appendFileSync, lstatSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { stopSignalProcesses, waitForSignalBoundary } from './signal-process.mjs';

// Discard syscall payloads. Preserve only fixed protocol identifiers and outcomes;
// arguments, graph contents, credentials and arbitrary error text are never saved.
export function controlTraceEvent(line) {
  const text = line.replace(/\\+"/g, '"');
  const code = text.match(/"code"\s*:\s*"([a-z][a-z0-9_]{0,63})"/)?.[1];
  if (!['control_unavailable', 'control_request_failed', 'graph_revision_stale', 'node_not_found'].includes(code)) return null;
  const statuses = [...text.matchAll(/"status"\s*:\s*([1-5][0-9]{2})\b/g)].map(match => Number(match[1]));
  if (!statuses.length) return null;
  const outcomes = Object.fromEntries([...text.matchAll(
    /"(operationOutcome|nodeControlOutcome|catalogOutcome|powerOutcome|workspaceOutcome)"\s*:\s*"(not_applied|pending|uncertain)"/g,
  )].map(match => [match[1], match[2]]));
  return { code, statuses, outcomes };
}

export function controlTraceStatuses(line) {
  return [...line.replace(/\\+"/g, '"').matchAll(/"status"\s*:\s*([1-5][0-9]{2})\b/g)]
    .map(match => Number(match[1]));
}

export async function startLinuxControlTrace(output) {
  assert.equal(process.platform, 'linux');
  assert.equal(process.env.GITHUB_ACTIONS, 'true', 'Control tracing requires a disposable CI guest');
  assert.equal(process.env.MIXRELAY_E2E_CONTROL_TRACE, 'true');
  const owners = readdirSync('/proc').filter(id => /^\d+$/.test(id)).filter(id => {
    try {
      return lstatSync(`/proc/${id}`).uid === process.getuid()
        && readFileSync(`/proc/${id}/cmdline`).toString() === '/usr/bin/mixrelay-desktop\0--driver-runtime\0';
    } catch (error) { if (!['ENOENT', 'ESRCH'].includes(error.code)) throw error; return false; }
  });
  assert.equal(owners.length, 1, 'Select exactly one owned Driver runtime for control diagnosis');
  const pid = owners[0];
  const file = join(output, 'control-response-trace.jsonl');
  writeFileSync(file, JSON.stringify({ event: 'starting', pid: Number(pid), scope: 'UI-only diagnostic' }) + '\n', { flag: 'wx' });
  // UnixStream::write uses send(MSG_NOSIGNAL), exposed as sendto on Linux.
  const child = spawn('sudo', ['-n', 'strace', '-f', '-ttt', '-s', '65536', '-e', 'trace=write,writev,sendto,sendmsg', '-p', pid],
    { stdio: ['ignore', 'ignore', 'pipe'] });
  const statuses = {};
  let attached = false, closed = false, failure, partial = '';
  const note = event => appendFileSync(file, JSON.stringify(event) + '\n');
  child.on('error', error => { failure = error; });
  child.once('close', (code, signal) => { closed = true; note({ event: 'closed', code, signal }); });
  child.stderr.on('data', bytes => {
    partial += bytes.toString();
    if (partial.length > 1024 * 1024) {
      failure = new Error('Control trace exceeded its bounded line storage');
      partial = '';
      child.kill('SIGTERM');
      return;
    }
    for (;;) {
      const end = partial.indexOf('\n');
      if (end < 0) break;
      const line = partial.slice(0, end); partial = partial.slice(end + 1);
      if (line.includes(`Process ${pid} attached`)) attached = true;
      for (const status of controlTraceStatuses(line)) statuses[status] = (statuses[status] ?? 0) + 1;
      const event = controlTraceEvent(line);
      if (event) note({ event: 'response', at: new Date().toISOString(), ...event });
      if (/strace:.*(?:Operation not permitted|Permission denied|invalid system call)/.test(line)) {
        failure = new Error('Control trace attachment was rejected');
      }
    }
  });
  const stop = async () => {
    await stopSignalProcesses([child]);
    await waitForSignalBoundary(() => closed, 'Control trace pipe close', { timeoutMs: 5000 });
    note({ event: 'status-counts', statuses });
    assert.ifError(failure);
    // Detaching must leave the original controller alive before native audio starts.
    assert.ok(readFileSync(`/proc/${pid}/cmdline`).length > 0, 'Tracing lost the owned controller');
    assert.match(readFileSync(`/proc/${pid}/status`, 'utf8'), /^TracerPid:\s+0$/m,
      'The controller must be detached before native audio');
  };
  try {
    await waitForSignalBoundary(() => {
      assert.ifError(failure);
      assert.equal(closed, false, 'Control trace exited before attachment');
      return attached;
    }, 'Control trace attachment', { timeoutMs: 10000 });
    note({ event: 'attached', pid: Number(pid) });
    return { stop };
  } catch (error) { await stop(); throw error; }
}
