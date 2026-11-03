import assert from 'node:assert/strict';
import test from 'node:test';
import { InspectorModel } from './inspector-model.mjs';
import { actionIds } from './action-settings.mjs';

test('changing graphs clears the old node and disconnected or missing choices retain no synthetic targets', () => {
  const model = new InspectorModel(actionIds.recording, { graphId: 'old', nodeId: 'take' }, false, 'en');
  assert.deepEqual(model.view().graphs, []);
  assert.deepEqual(model.select('graph', 'g'), { graphId: 'g', nodeId: '' });
  model.snapshot = { graphs: [{ id: 'g', nodes: [
    { nodeId: 'take', capabilities: { recorder: true } },
    { nodeId: 'bus', capabilities: { power: true } },
  ] }] };
  assert.deepEqual(model.view().nodes.map((node) => node.nodeId), ['take']);
  assert.equal(model.view().selected.operation, 'record');
  model.select('node', ' take ');
  model.select('operation', 'pause');
  assert.equal(model.settings.nodeId, 'take');
  assert.equal(model.view().selected.operation, 'pause');
  model.snapshot = undefined;
  assert.deepEqual(model.view().nodes, []);
  assert.equal(model.settings.nodeId, 'take');
});

test('dial volume visibility follows action identity and settings stay scoped to their inspector', () => {
  const dial = new InspectorModel(actionIds.volume, {}, true, 'en');
  const recorder = new InspectorModel(actionIds.recording, {}, false, 'en');
  assert.equal(dial.view().hint, 'Rotate: volume · Press: mute.');
  assert.equal(dial.view().volume, true);
  assert.equal(dial.view().recording, false);
  dial.select('step', '2');
  assert.equal(dial.settings.stepDb, 2);
  assert.equal(recorder.settings.stepDb, undefined);
});
