import assert from 'node:assert/strict';
import test from 'node:test';
import { operations } from './capabilities.mjs';
import { commandFor } from './control-command.mjs';
import { presentation } from './controls.mjs';

const node = {
  nodeId: 'n',
  control: { enabled: true, muted: false },
  capabilities: { power: true, mute: true, recorder: false },
  gainDb: 0,
};
const snapshot = (target = node) => ({
  session: 'session-a',
  revision: 8,
  activeRevision: 8,
  graphs: [{ id: 'g', nodes: [target] }],
});
const settings = { graphId: 'g', nodeId: 'n', operation: 'power' };

test('power and mute use observed absolute state and graph revision', () => {
  assert.deepEqual(commandFor(snapshot(), settings), {
    type: 'node',
    session: 'session-a',
    target: { graphId: 'g', nodeId: 'n' },
    request: { expectedRevision: 8, enabled: false },
  });
  assert.equal(commandFor(snapshot(), settings, 'mute').request.muted, true);
});
test('a desired state is not displayed as active or used for another command before acknowledgement', () => {
  const pending = { ...snapshot(), activeRevision: 7 };
  assert.equal(presentation(pending, settings).value, 'Pending');
  assert.throws(() => commandFor(pending, settings), /Target unavailable/);
});
test('off node permits power only; missing and disconnected targets reject', () => {
  const off = snapshot({ ...node, control: { enabled: false, muted: false } });
  assert.equal(commandFor(off, settings).request.enabled, true);
  for (const operation of ['mute', 'gain_up', 'record'])
    assert.throws(() => commandFor(off, settings, operation));
  assert.throws(() => commandFor(undefined, settings));
  assert.throws(() => commandFor({ session: 'a', revision: 8, graphs: [] }, settings));
  assert.equal(presentation(undefined, settings).value, 'Disconnected');
});
test('discovery does not offer recorder controls for other nodes', () => {
  assert.deepEqual(operations(node), ['power', 'mute', 'gain_up', 'gain_down']);
  assert.deepEqual(operations(undefined), []);
});
test('dial increments are bounded and reject nonfinite input', () => {
  assert.equal(commandFor(snapshot(), settings, 'gain_up', -5).request.gainDb, -2.5);
  assert.equal(commandFor(snapshot(), settings, 'gain_up', 999).request.gainDb, 12);
  assert.equal(commandFor(snapshot(), settings, 'gain_down', 999).request.gainDb, -60);
  assert.throws(() => commandFor(snapshot(), settings, 'gain_up', NaN));
});
test('recording commands carry observed take identity and state', () => {
  const recorder = {
    ...node,
    capabilities: { ...node.capabilities, recorder: true },
    recorder: { state: 'recording', path: '/fixture/take-a.wav' },
  };
  assert.deepEqual(commandFor(snapshot(recorder), settings, 'stop').request.expectedRecording, {
    state: 'recording',
    path: '/fixture/take-a.wav',
  });
  assert.throws(() => commandFor(snapshot(recorder), settings, 'record'));
  recorder.recorder = { state: 'paused', path: '/fixture/take-a.wav' };
  assert.deepEqual(commandFor(snapshot(recorder), settings, 'record').request.request, {
    action: 'record',
  });
  recorder.recorder = { state: 'stopped', path: null };
  const take = commandFor(snapshot(recorder), settings, 'record').request;
  assert.deepEqual(take.request, { action: 'record' });
  assert.deepEqual(take.expectedRecording, { state: 'stopped', path: null });
});
