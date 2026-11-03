import assert from 'node:assert/strict';
import test from 'node:test';
import { VolumeGesture } from './volume-gesture.mjs';

const settings = { graphId: 'g', nodeId: 'n', stepDb: 0.5 };
function fixture(gainDb = 0) {
  const gate = Promise.withResolvers();
  const writes = [];
  const node = {
    nodeId: 'n',
    gainDb,
    control: { enabled: true, muted: false },
    capabilities: { power: true, mute: true, recorder: false },
  };
  const snapshot = {
    session: 'a',
    revision: 1,
    activeRevision: 1,
    structureGeneration: 1,
    graphs: [{ id: 'g', nodes: [node] }],
  };
  const session = {
    snapshot,
    refresh: async () => {},
    setGain: async (_settings, gain) => {
      writes.push(gain);
      if (writes.length === 1) await gate.promise;
      node.gainDb = gain;
      snapshot.revision += 1;
      snapshot.activeRevision = snapshot.revision;
      return { revision: snapshot.revision };
    },
  };
  let now = 100;
  const gesture = new VolumeGesture(
    session,
    () => now,
    async () => {
      now += 25;
    },
  );
  return {
    gate,
    writes,
    node,
    snapshot,
    session,
    gesture,
    advance: (ms) => {
      now += ms;
    },
  };
}

test('rapid encoder events retain every signed tick while a write is in flight', async () => {
  const f = fixture();
  const first = f.gesture.change('dial', settings, 2);
  await Promise.resolve();
  const second = f.gesture.change('dial', settings, 3);
  const third = f.gesture.change('dial', settings, -1);
  assert.equal(second, first);
  assert.equal(third, first);
  assert.deepEqual(f.writes, [1]);
  f.gate.resolve();
  await first;
  assert.deepEqual(f.writes, [1, 2]);
  assert.equal(f.node.gainDb, 2);
});

test('each event clamps before reversing so saturation never swallows reverse ticks', async () => {
  const f = fixture(11);
  const task = f.gesture.change('dial', settings, 1);
  await Promise.resolve();
  f.gesture.change('dial', settings, 20);
  f.gesture.change('dial', settings, -1);
  f.gate.resolve();
  await task;
  assert.equal(f.node.gainDb, 11.5);
  assert.deepEqual(f.writes, [11.5]);
});

test('continuous input extends the gesture lifetime, without extending a stalled acknowledgement forever', async () => {
  const f = fixture();
  const task = f.gesture.change('dial', settings, 1);
  await Promise.resolve();
  f.advance(650);
  f.gesture.change('dial', settings, 1);
  f.advance(50);
  f.gate.resolve();
  await task;
  assert.deepEqual(f.writes, [0.5, 1]);
});

for (const reason of [
  'off',
  'deleted',
  'session',
  'structure',
  'revision',
  'expired',
  'cancelled',
  'disconnected',
]) {
  test(`${reason} discards pending volume intent without replay`, async () => {
    const f = fixture();
    const task = f.gesture.change('dial', settings, 1);
    await Promise.resolve();
    f.gesture.change('dial', settings, 2);
    if (reason === 'off') f.node.control.enabled = false;
    if (reason === 'deleted') f.snapshot.graphs[0].nodes = [];
    if (reason === 'session') f.snapshot.session = 'other';
    if (reason === 'structure') f.snapshot.structureGeneration += 1;
    if (reason === 'revision') f.snapshot.revision += 1;
    if (reason === 'expired') f.advance(751);
    if (reason === 'cancelled') f.gesture.cancel('dial');
    if (reason === 'disconnected') f.session.snapshot = undefined;
    f.gate.resolve();
    await assert.rejects(task);
    assert.equal(f.writes.length, 1);
  });
}

test('rejected or uncertain writes never drain the pending gesture', async () => {
  const f = fixture();
  const task = f.gesture.change('dial', settings, 1);
  await Promise.resolve();
  f.gesture.change('dial', settings, 2);
  f.gate.reject(new Error('lost response'));
  await assert.rejects(task, /lost response/);
  assert.equal(f.writes.length, 1);
});

test('settings retarget cancels trailing intent instead of sending it to a replacement node', async () => {
  const f = fixture();
  const task = f.gesture.change('dial', settings, 1);
  await Promise.resolve();
  await assert.rejects(f.gesture.change('dial', { ...settings, nodeId: 'other' }, 1));
  f.gate.resolve();
  await assert.rejects(task);
  assert.equal(f.writes.length, 1);
});

test('input after an expired silence cannot resurrect old pending intent', async () => {
  const f = fixture();
  const task = f.gesture.change('dial', settings, 1);
  await Promise.resolve();
  f.advance(751);
  await assert.rejects(f.gesture.change('dial', settings, 1), /expired/);
  f.gate.resolve();
  await assert.rejects(task);
  assert.equal(f.writes.length, 1);
});

test('continuous ticks cannot extend the fixed deadline of one stalled write', async () => {
  const f = fixture();
  const task = f.gesture.change('dial', settings, 1);
  await Promise.resolve();
  for (let index = 0; index < 3; index += 1) {
    f.advance(300);
    f.gesture.change('dial', settings, 1);
  }
  f.gate.resolve();
  await assert.rejects(task, /timed out/);
  assert.equal(f.writes.length, 1);
});

test('pending active acknowledgement is read until confirmed before trailing gain dispatch', async () => {
  const f = fixture();
  const send = f.session.setGain;
  let reads = 0;
  f.session.setGain = async (...args) => {
    const result = await send(...args);
    f.snapshot.activeRevision -= 1;
    return result;
  };
  f.session.refresh = async () => {
    reads += 1;
    f.snapshot.activeRevision = f.snapshot.revision;
  };
  const task = f.gesture.change('dial', settings, 1);
  await Promise.resolve();
  f.gesture.change('dial', settings, 1);
  f.gate.resolve();
  await task;
  assert.deepEqual(f.writes, [0.5, 1]);
  assert.equal(reads, 2);
});

test('late refresh cannot drain intent after the fixed write deadline even with fresh input', async () => {
  const f = fixture();
  const send = f.session.setGain;
  f.session.setGain = async (...args) => {
    const response = await send(...args);
    f.snapshot.activeRevision -= 1;
    return response;
  };
  f.session.refresh = async () => {
    f.advance(700);
    f.gesture.change('dial', settings, 1);
    f.advance(100);
    f.snapshot.activeRevision = f.snapshot.revision;
  };
  const task = f.gesture.change('dial', settings, 1);
  await Promise.resolve();
  f.gesture.change('dial', settings, 1);
  f.gate.resolve();
  await assert.rejects(task, /timed out/);
  assert.equal(f.writes.length, 1);
});
