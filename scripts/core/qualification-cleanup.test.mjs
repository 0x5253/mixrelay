import assert from 'node:assert/strict';
import test from 'node:test';
import { restoreQualificationGraph, finishAfterCleanup } from './qualification-cleanup.mjs';

const owned = { original: { revision: 1, nodes: [] }, applied: { revision: 3, nodes: [{ id: 'owned' }] },
  unconfirmedMutation: false, graphId: 'g', recorderPath: 'recorders/r' };

test('restoration stops the observed take before publishing the original graph at the next revision', async () => {
  const calls = [];
  await restoreQualificationGraph(owned, {
    api: async (path) => path === 'graphs' ? { activeId: 'g', document: owned.applied } : { state: 'paused', path: '/take.wav' },
    controlTake: async (request) => calls.push(request),
    apply: async (document) => calls.push(document),
  });
  assert.deepEqual(calls, [
    { expectedRevision: 3, expectedRecording: { state: 'paused', path: '/take.wav' }, request: { action: 'stop' } },
    { revision: 4, nodes: [], expectedGraphId: 'g' },
  ]);
});

for (const current of [{ activeId: 'other', document: owned.applied }, { activeId: 'g', document: { ...owned.applied, revision: 4 } }]) {
  test('concurrent selection or publication is preserved without recorder or graph changes', async () => {
    await assert.rejects(restoreQualificationGraph(owned, {
      api: async () => current,
      controlTake: async () => assert.fail('must preserve external recorder state'),
      apply: async () => assert.fail('must preserve external graph state'),
    }), /External/);
  });
}

test('an uncertain publication does not inspect or restore guessed state', async () => {
  await assert.rejects(restoreQualificationGraph({ ...owned, unconfirmedMutation: true }, {
    api: async () => assert.fail('uncertain ownership'),
  }), /Unconfirmed/);
});

test('UI observation is finalized after failed cleanup and the original failure is retained', async () => {
  const order = [];
  const failure = new Error('removal failed');
  await assert.rejects(finishAfterCleanup({ finish: async () => order.push('ui') }, async () => {
    order.push('cleanup'); throw failure;
  }), (error) => error === failure);
  assert.deepEqual(order, ['cleanup', 'ui']);
});
