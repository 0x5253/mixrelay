import assert from 'node:assert/strict';
import test from 'node:test';
import { controlRecorder } from './recorder-control.mjs';

const path = 'graphs/qa/recorders/owned';
const request = { expectedRevision: 8, expectedRecording: { state: 'recording', path: '/owned/take.wav' },
  request: { action: 'stop' } };
const busy = { status: 409, json: { code: 'graph_exchange_busy', parameters: { reason: 'graph mutation in progress' } } };
const current = async route => route === 'graphs'
  ? { activeId: 'qa', document: { revision: 8 } } : request.expectedRecording;

test('a rejected recorder command is admitted once after unchanged graph and take observations', async () => {
  let elapsed = 0, submissions = 0, applied = 0;
  const observations = [];
  const result = await controlRecorder(path, request, { graphId: 'qa', api: current,
    now: () => elapsed, wait: async ms => { elapsed += ms; }, observe: value => observations.push(value),
    operation: async (method, route, body) => {
      assert.equal(method, 'PUT'); assert.equal(route, `/api/v1/${path}`); assert.deepEqual(JSON.parse(body), request);
      if (++submissions < 3) return busy;
      applied++; return { status: 200, json: { state: 'stopped', error: null } };
    },
  });
  assert.equal(result.state, 'stopped'); assert.equal(applied, 1); assert.equal(observations.length, 2);
});

test('unknown, uncertain and other rejected recorder commands are never resubmitted', async () => {
  for (const response of [new Error('lost outcome'), { status: 500 },
    { status: 409, json: { code: 'graph_revision_stale' } },
    { status: 409, json: { code: 'graph_exchange_busy', parameters: { activeRevision: 7 } } }]) {
    let submissions = 0;
    await assert.rejects(controlRecorder(path, request, { graphId: 'qa', api: async () => assert.fail('No readback for an uncertain outcome'),
      now: () => 0, operation: async () => { submissions++; if (response instanceof Error) throw response; return response; },
    }));
    assert.equal(submissions, 1);
  }
});

test('changed selection, graph or recording prevents another recorder submission', async () => {
  for (const changed of ['selection', 'revision', 'recording']) {
    let submissions = 0;
    await assert.rejects(controlRecorder(path, request, { graphId: 'qa', now: () => 0,
      operation: async () => { submissions++; return busy; },
      api: async route => route === 'graphs'
        ? { activeId: changed === 'selection' ? 'other' : 'qa', document: { revision: changed === 'revision' ? 9 : 8 } }
        : changed === 'recording' ? { state: 'stopped', path: '/other/take.wav' } : request.expectedRecording,
    }), /changed/i);
    assert.equal(submissions, 1);
  }
});

test('permanent recorder admission failure remains bounded and fails', async () => {
  let elapsed = 0;
  await assert.rejects(controlRecorder(path, request, { graphId: 'qa', api: current,
    operation: async () => busy, now: () => elapsed, wait: async ms => { elapsed += ms; },
  }), /remained busy/);
  assert.equal(elapsed, 45000);
});
