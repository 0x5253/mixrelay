import assert from 'node:assert/strict';
import test from 'node:test';
import { actionIds } from './action-settings.mjs';
import { DeckControls } from './deck-controls.mjs';

function fixture() {
  const timers = [], frames = [], warnings = [], alerts = [], writes = [], published = [];
  const controls = new DeckControls({
    control: {
      send: async (command) => {
        writes.push(command);
        return {};
      },
      snapshot: async () => ({
        protocolVersion: 1, session: 's', revision: 1, activeRevision: 1, structureGeneration: 1,
        graphs: [{ id: 'g', nodes: [{
          nodeId: 'n', control: { enabled: true, muted: false },
          capabilities: { power: true, mute: true, recorder: false },
        }] }],
      }),
    },
    display: {
      render: async (entry, view) => frames.push([entry.action.id, view]),
      snapshot: async (snapshot) => published.push(snapshot),
      warn: (error) => warnings.push(error),
      alert: async (action) => alerts.push(action.id),
    },
    now: () => 1000,
    wait: async () => {},
    schedule: (callback) => { timers.push(callback); return timers.length; },
  });
  const action = { id: 'key', manifestId: actionIds.power, isDial: () => false };
  return { controls, action, timers, frames, warnings, alerts, writes, published };
}

test('action lifecycle publishes through display ports and removed actions cannot issue controls', async () => {
  const f = fixture();
  f.controls.update(f.action, { graphId: 'g', nodeId: 'n' });
  await f.controls.session.refresh();
  await f.timers.shift()();
  assert.equal(f.frames.length, 1);
  assert.equal(f.frames[0][0], 'key');
  assert.equal(f.published[0].session, 's');
  await f.controls.control('key');
  assert.equal(f.writes.length, 1);
  assert.equal(f.writes[0].request.enabled, false);
  f.controls.remove('key');
  await f.controls.control('key');
  await f.timers.shift()();
  assert.equal(f.writes.length, 1);
  assert.equal(f.frames.length, 1);
});

test('invalid dial intent alerts its action without dispatching a mutation', async () => {
  const f = fixture();
  f.controls.update(f.action, { graphId: 'g', nodeId: 'n' });
  await f.controls.session.refresh();
  await f.controls.control('key', 'mute');
  assert.deepEqual(f.alerts, ['key']);
  assert.match(f.warnings[0].message, /does not support dial/);
  assert.deepEqual(f.writes, []);
});
