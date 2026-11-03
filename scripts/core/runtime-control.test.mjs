import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import test from 'node:test';
import { runtimeControl } from './runtime-control.mjs';

function fixture() {
  const socket = new EventEmitter(), writes = [], timers = new Map();
  let sequence = 0, closed = 0;
  socket.write = (frame) => writes.push(JSON.parse(frame));
  socket.destroy = () => closed++;
  const invoke = runtimeControl({
    connect: (path) => { assert.equal(path, '/owned.sock'); return socket; },
    path: '/owned.sock',
    schedule: (callback, delay) => { assert.equal(delay, 20000); timers.set(++sequence, callback); return sequence; },
    cancel: (id) => timers.delete(id),
  });
  return { socket, writes, timers, invoke, closed: () => closed };
}

test('fragmented UTF-8 pending and final runtime replies preserve bytes and invoke only once', async () => {
  const f = fixture();
  const result = f.invoke('read', { value: 1 });
  f.socket.emit('connect');
  const reply = Buffer.from('{"id":1,"status":102}\n{"id":1,"status":200,"body":{"ok":{"label":"녹음"}}}\n');
  const split = reply.indexOf(Buffer.from('녹음')) + 1;
  f.socket.emit('data', reply.subarray(0, split));
  f.socket.emit('data', reply.subarray(split));
  assert.deepEqual(await result, { label: '녹음' });
  assert.deepEqual(f.writes, [{ id: 1, command: { type: 'invoke', name: 'read', arguments: { value: 1 } } }]);
  assert.equal(f.timers.size, 0);
  assert.equal(f.closed(), 1);
});

test('partial replies cannot renew the deadline or replay uncertain mutations', async () => {
  const f = fixture();
  const result = f.invoke('mutate', {});
  f.socket.emit('connect');
  f.socket.emit('data', Buffer.from('{"id":1,"status":102'));
  assert.deepEqual([...f.timers.keys()], [1]);
  f.timers.get(1)();
  await assert.rejects(result, /timed out; inspect graph before retrying/);
  assert.equal(f.writes.length, 1);
  assert.equal(f.closed(), 1);
});

for (const reply of ['{"id":2,"status":200,"body":{"ok":true}}\n', '{"id":1,"status":200,"body":{"error":"failure"}}\n']) {
  test('invalid runtime envelope fails without replay', async () => {
    const f = fixture();
    const result = f.invoke('mutate', {});
    f.socket.emit('connect');
    f.socket.emit('data', Buffer.from(reply));
    await assert.rejects(result);
    assert.equal(f.writes.length, 1);
    assert.equal(f.closed(), 1);
  });
}
