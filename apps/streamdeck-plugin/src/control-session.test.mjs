import assert from 'node:assert/strict';
import test from 'node:test';
import { ControlSession } from './control-session.mjs';

function sessionFor(exchange, publish, now) {
  return new ControlSession({
    snapshot: () => exchange({ type: 'snapshot' }),
    send: exchange,
  }, publish, now);
}

const settings = { graphId: 'g', nodeId: 'n', operation: 'power' };
const snapshot = (revision = 1, enabled = true) => ({
  protocolVersion: 1,
  session: 'session-a',
  revision,
  activeRevision: revision,
  structureGeneration: 1,
  graphs: [
    {
      id: 'g',
      nodes: [
        {
          nodeId: 'n',
          control: { enabled, muted: false },
          capabilities: { power: true, mute: true, recorder: false },
        },
      ],
    },
  ],
});

test('a polling read does not reject a fresh button press or publish its stale completion', async () => {
  const pending = Promise.withResolvers();
  const published = [];
  const commands = [];
  let reads = 0;
  const session = sessionFor(
    async (command) => {
      if (command.type !== 'snapshot') {
        commands.push(command);
        return {};
      }
      reads += 1;
      return reads === 2 ? pending.promise : snapshot(reads === 1 ? 1 : 2, reads === 1);
    },
    () => published.push(session.snapshot?.revision),
    () => 1000,
  );
  await session.refresh();
  const polling = session.refresh();
  const write = session.control(settings);
  assert.equal(commands.length, 1);
  assert.equal(commands[0].request.expectedRevision, 1);
  pending.resolve(snapshot());
  await Promise.all([polling, write]);
  assert.deepEqual(published, [1, 2]);
  assert.equal(session.snapshot.graphs[0].nodes[0].control.enabled, false);
});

test('overlapping and stale commands are rejected, never queued or rebased', async () => {
  const pending = Promise.withResolvers();
  let writes = 0;
  let time = 1000;
  const session = sessionFor(
    async (command) => {
      if (command.type === 'snapshot') return snapshot();
      writes += 1;
      return pending.promise;
    },
    () => {},
    () => time,
  );
  await session.refresh();
  const first = session.control(settings);
  await assert.rejects(session.control(settings), /busy or stale/);
  assert.equal(writes, 1);
  pending.resolve({});
  await first;
  time += 2501;
  await assert.rejects(session.control(settings), /busy or stale/);
  assert.equal(writes, 1);
});

test('lost command response is surfaced once and disconnect invalidates controls', async () => {
  let calls = 0;
  const session = sessionFor(
    async () => {
      calls += 1;
      if (calls === 1) return snapshot();
      throw new Error('connection lost');
    },
    () => {},
    () => 1000,
  );
  await session.refresh();
  await assert.rejects(session.control(settings), /connection lost/);
  assert.equal(calls, 3);
  assert.equal(session.snapshot, undefined);
  await assert.rejects(session.control(settings));
  assert.equal(calls, 3);
});

test('one shared snapshot read replaces per-graph and per-recorder fan-out', async () => {
  const pending = Promise.withResolvers();
  let calls = 0;
  const session = sessionFor(
    async (command) => {
      assert.deepEqual(command, { type: 'snapshot' });
      calls += 1;
      return pending.promise;
    },
    () => {},
    () => 1000,
  );
  const first = session.refresh();
  const second = session.refresh();
  assert.equal(first, second);
  pending.resolve({
    ...snapshot(),
    graphs: Array.from({ length: 20 }, (_, id) => ({
      id: String(id),
      nodes: Array.from({ length: 20 }, () => ({ recorder: { state: 'stopped' } })),
    })),
  });
  await first;
  assert.equal(calls, 1);
  assert.equal(session.snapshot.graphs.length, 20);
});
