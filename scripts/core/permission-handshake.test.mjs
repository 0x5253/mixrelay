import assert from 'node:assert/strict';
import test from 'node:test';
import { permissionHandshake } from './permission-handshake.mjs';

function fixture(respond) {
  let id = 0, elapsed = 0, command;
  const receipts = [], commands = [];
  const boundary = permissionHandshake({
    directory: '/evidence', alive: () => true,
    now: () => elapsed, wait: async () => { elapsed += 100; }, nextId: () => String(++id),
    channel: {
      publish: (next) => { command = next; commands.push(next); },
      read: () => respond(command, elapsed),
    },
    writeReceipt: (directory, value) => receipts.push({ directory, value }),
  });
  return { boundary, receipts, commands };
}

test('measurement starts only after the matching pause generation and resumes before writing its receipt', async () => {
  const f = fixture((command, elapsed) => elapsed === 0
    ? { ...command, generation: 'old', at: '2026-11-02T00:00:00Z' }
    : { ...command, at: '2026-11-02T00:00:00Z' });
  const result = await f.boundary(async () => {
    assert.deepEqual(f.commands, [{ generation: '1', phase: 'paused' }]); return 'measured';
  });
  assert.equal(result, 'measured');
  assert.deepEqual(f.commands.map((value) => value.phase), ['paused', 'observing']);
  assert.equal(f.receipts[0].value.resumed.generation, '2');
});

test('measurement and resume failures are retained together and still produce a receipt', async () => {
  const f = fixture((command) => command.phase === 'paused'
    ? { ...command, at: '2026-11-02T00:00:00Z' } : undefined);
  const primary = new Error('PCM measurement failed');
  await assert.rejects(f.boundary(async () => { throw primary; }), (error) => {
    assert.ok(error instanceof AggregateError);
    assert.equal(error.errors[0], primary);
    assert.match(error.errors[1].message, /did not acknowledge observing/);
    return true;
  });
  assert.equal(f.receipts.length, 1);
  assert.equal(f.receipts[0].value.resumed, undefined);
});
