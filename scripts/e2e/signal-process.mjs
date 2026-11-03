import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';

export async function stopSignalProcesses(children, graceMs = 5000) {
  const live = child => child.exitCode === null && child.signalCode === null;
  const wait = child => new Promise(resolve => {
    if (!live(child)) { resolve(); return; }
    const done = () => { clearTimeout(timer); child.removeListener('exit', done); resolve(); };
    const timer = setTimeout(done, graceMs);
    child.once('exit', done);
  });
  const failures = [];
  for (const child of children.filter(Boolean)) {
    if (live(child)) child.kill('SIGTERM');
    await wait(child);
    if (live(child)) {
      child.kill('SIGKILL');
      await wait(child);
      failures.push(live(child) ? 'Native signal process did not stop after SIGKILL' : 'Native signal process required SIGKILL');
    }
  }
  if (failures.length) throw new Error(failures.join('; '));
}

export async function waitForSignalBoundary(read, description, {
  timeoutMs = 30000, now = Date.now, wait = delay,
} = {}) {
  const deadline = now() + timeoutMs;
  for (;;) {
    assert.ok(now() < deadline, `${description} did not complete before its deadline`);
    if (read()) return;
    await wait(50);
  }
}
