import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { stopSignalProcesses } from './signal-process.mjs';

// Registration is acknowledged before xctrace starts, so its notification cannot
// race observer startup. The disposable helper owns no audio or installed state.
export function startTraceNotification({ execute = execFileSync, start = spawn } = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'mr-trace-notify-'));
  const executable = join(directory, 'observer');
  const name = `io.mixrelay.diagnostics.trace.${randomUUID()}`;
  let child;
  try {
    execute('xcrun', ['clang', '-std=c17', '-fblocks', '-Wall', '-Wextra', '-Wpedantic', '-Werror',
      fileURLToPath(new URL('./macos-trace-notification.c', import.meta.url)), '-o', executable],
    { encoding: 'utf8', timeout: 30000 });
    child = start(executable, [name], { stdio: ['ignore', 'pipe', 'pipe'] });
  } catch (error) { rmSync(directory, { recursive: true, force: true }); throw error; }
  let log = '', closed = false, failure, stopping = false;
  child.on('error', error => { failure ??= error; });
  child.once('close', (code, signal) => {
    closed = true;
    if (!stopping && code !== 0) failure ??= new Error(`Trace observer exited with code ${code}, signal ${signal}`);
  });
  const observe = bytes => {
    if (log.length + bytes.length > 4096) failure ??= new Error('Trace observer exceeded its bounded output');
    else log += bytes.toString();
  };
  child.stdout.on('data', observe);
  child.stderr.on('data', observe);
  const observed = marker => {
    assert.ifError(failure);
    const found = log.split(/\r?\n/).includes(marker);
    assert.ok(found || !closed, `Trace observer ended before ${marker}`);
    return found;
  };
  return {
    name, registered: () => observed('registered'), started: () => observed('started'), log: () => log,
    async stop() {
      stopping = true;
      try { await stopSignalProcesses([child]); }
      finally { rmSync(directory, { recursive: true, force: true }); }
    },
  };
}
