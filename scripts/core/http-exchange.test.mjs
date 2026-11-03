import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import test from 'node:test';
import { httpExchange } from './http-exchange.mjs';

function fixture() {
  const outgoing = new EventEmitter(), response = new EventEmitter();
  let timeout, cancelled, requests = 0, destroyed = 0;
  outgoing.end = () => {};
  outgoing.destroy = () => destroyed++;
  response.destroy = () => destroyed++;
  response.setEncoding = () => {};
  response.statusCode = 200;
  response.headers = {};
  let connected;
  const exchange = httpExchange({
    request: (_options, callback) => { requests++; connected = callback; return outgoing; },
    schedule: (callback, delay) => { timeout = callback; assert.equal(delay, 100); return 7; },
    cancel: (timer) => { cancelled = timer; },
  });
  return { outgoing, response, exchange,
    connect: () => connected(response), expire: () => timeout(),
    observed: () => ({ cancelled, requests, destroyed }),
  };
}

test('continuous body traffic cannot extend the absolute deadline or replay a request', async () => {
  const f = fixture();
  const pending = f.exchange('/socket', '/graph', { timeoutMs: 100 });
  f.connect();
  for (let index = 0; index < 10; index++) f.response.emit('data', 'still receiving');
  f.expire();
  await assert.rejects(pending, /timed out/);
  assert.deepEqual(f.observed(), { cancelled: 7, requests: 1, destroyed: 2 });
});

test('response byte limits reject multibyte data and completion cancels the deadline', async () => {
  const f = fixture();
  const pending = f.exchange('/socket', '/status', { timeoutMs: 100, responseLimit: 5 });
  f.connect();
  f.response.emit('data', '녹음');
  await assert.rejects(pending, /response too large/);
  const good = fixture();
  const success = good.exchange('/socket', '/status', { timeoutMs: 100 });
  good.connect();
  good.response.emit('data', 'ok');
  good.response.emit('end');
  assert.deepEqual(await success, { status: 200, headers: {}, body: 'ok' });
  assert.deepEqual(good.observed(), { cancelled: 7, requests: 1, destroyed: 0 });
  good.expire();
  assert.equal(good.observed().destroyed, 0);
});
