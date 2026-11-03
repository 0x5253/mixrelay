import assert from 'node:assert/strict';
import test from 'node:test';
import { driverOperations } from './driver-operations.mjs';

const busy = { status: 503, body: 'Driver control ingress is full' };
const reply = revision => ({ status: 200, body: JSON.stringify({ revision }) });
const commandOf = (path, options) => {
  assert.equal(path, '/api/v1/driver');
  assert.equal(options.headers.host, 'mixrelay-driver');
  if (options.method === 'GET') { assert.equal(options.body, undefined); return undefined; }
  assert.equal(options.method, 'PUT');
  assert.equal(options.headers['content-type'], 'application/json');
  return JSON.parse(options.body);
};

test('Driver admission recovery observes the unchanged revision and applies a rejected mutation once', async () => {
  let attempts = 0, applied = 0;
  const calls = [], observed = [];
  const driver = driverOperations({ wait: async () => {}, observe: item => observed.push(item),
    request: async (path, options) => {
      const command = commandOf(path, options);
      calls.push(command);
      if (!command) return reply(7);
      if (++attempts === 1) return busy;
      applied++;
      return reply(8);
    } });
  const command = { action: 'configure', expectedRevision: 7 };
  assert.deepEqual(await driver(command), { revision: 8 });
  assert.deepEqual(calls, [command, undefined, command]);
  assert.equal(applied, 1);
  assert.equal(observed.length, 1);
});

test('Driver admission cannot replay a mutation after another owner changes the revision', async () => {
  let attempts = 0;
  const driver = driverOperations({ request: async (path, options) => commandOf(path, options) ? (++attempts, busy) : reply(8) });
  await assert.rejects(driver({ action: 'configure', expectedRevision: 7 }), /Driver changed/);
  assert.equal(attempts, 1);
});

test('Driver transport uncertainty and other server failures never resubmit a mutation', async () => {
  for (const failure of [new Error('reply lost'), { status: 503, body: 'backend unavailable' }]) {
    let calls = 0;
    const driver = driverOperations({ request: async () => {
      calls++;
      if (failure instanceof Error) throw failure;
      return failure;
    } });
    await assert.rejects(driver({ action: 'configure', expectedRevision: 7 }));
    assert.equal(calls, 1);
  }
});

test('a lost Driver reply identifies the attempted action without replaying it', async () => {
  let calls = 0;
  const observations = [];
  const driver = driverOperations({ now: () => 42, observe: item => observations.push(item),
    request: async () => {
      calls++;
      throw Object.assign(new Error('socket hang up'), { code: 'ECONNRESET' });
    } });
  await assert.rejects(driver({ action: 'configure', expectedRevision: 7 }),
    /Driver PUT configure transport failed \(ECONNRESET\): socket hang up/);
  assert.equal(calls, 1);
  assert.deepEqual(observations, [{ at: 42, action: 'configure', method: 'PUT',
    failure: 'transport', code: 'ECONNRESET', message: 'socket hang up' }]);
});

test('persistent Driver ingress rejection is bounded for reads and mutations', async () => {
  for (const command of [undefined, { action: 'configure', expectedRevision: 7 }]) {
    let elapsed = 0, calls = 0;
    const driver = driverOperations({ now: () => elapsed, wait: async ms => { elapsed += ms; }, timeoutMs: 500,
      request: async () => { calls++; return busy; } });
    await assert.rejects(driver(command), /deadline/);
    assert.ok(calls <= 4);
  }
});
