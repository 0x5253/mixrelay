import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { stopSignalProcesses, waitForSignalBoundary } from './signal-process.mjs';
import { startTraceNotification } from './macos-trace-notification.mjs';

// Record the running system independently of LaunchServices: profiling `open`
// alone can end before the launched application starts its measurement.
function startRecorder(directory, name, start) {
  const child = start('xcrun', ['xctrace', 'record', '--template', 'System Trace', '--no-prompt',
    '--all-processes', '--notify-tracing-started', name,
    '--output', join(directory, 'scheduler.trace'), '--time-limit', '45s', '--window', '45s'],
  { stdio: ['ignore', 'pipe', 'pipe'] });
  let log = '', closed = false, failure;
  child.on('error', error => { failure ??= error; });
  child.once('close', (code, signal) => {
    closed = true;
    if (code !== 0) failure ??= new Error(`System Trace exited with code ${code}, signal ${signal}`);
  });
  const observe = bytes => {
    if (log.length + bytes.length > 1024 * 1024) {
      failure ??= new Error('System Trace exceeded its bounded diagnostic output');
      child.kill('SIGINT');
    } else log += bytes.toString();
  };
  child.stdout.on('data', observe);
  child.stderr.on('data', observe);
  return { child, log: () => log, closed: () => closed, failure: () => failure };
}

export async function withMacosSchedulerTrace(directory, measure, {
  start = spawn, wait = waitForSignalBoundary, notify = startTraceNotification,
} = {}) {
  assert.equal(process.env.GITHUB_ACTIONS, 'true', 'Scheduler diagnosis requires a disposable hosted Mac');
  const notification = notify();
  const errors = [];
  let recorder;
  let result;
  try {
    await wait(notification.registered, 'System Trace observer registration', { timeoutMs: 10000 });
    recorder = startRecorder(directory, notification.name, start);
    await wait(() => {
      assert.ifError(recorder.failure());
      assert.equal(recorder.closed(), false, 'System Trace exited before measurement');
      return notification.started();
    }, 'System Trace readiness', { timeoutMs: 180000 });
    result = await measure();
    assert.equal(recorder.closed(), false, 'System Trace ended before measurement completed');
  } catch (error) { errors.push(error); }
  try {
    if (recorder) {
      if (!recorder.closed()) recorder.child.kill('SIGINT');
      await wait(recorder.closed, 'System Trace finalization', { timeoutMs: 90000 });
      assert.ifError(recorder.failure());
    }
  } catch (error) {
    errors.push(error);
    try { await stopSignalProcesses([recorder.child]); } catch (stopError) { errors.push(stopError); }
  } finally {
    try { await notification.stop(); } catch (error) { errors.push(error); }
    writeFileSync(join(directory, 'scheduler.log'), recorder?.log() ?? '', { flag: 'wx' });
    writeFileSync(join(directory, 'scheduler-notification.log'), notification.log(), { flag: 'wx' });
  }
  if (errors.length) throw new AggregateError(errors, errors.map(error => error.message).join('; '));
  return result;
}
