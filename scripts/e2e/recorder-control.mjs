import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';

export async function controlRecorder(path, request, {
  api, operation, graphId, now = Date.now, wait = delay, observe = () => {},
}) {
  const deadline = now() + 45000;
  while (now() < deadline) {
    const reply = await operation('PUT', `/api/v1/${path}`, JSON.stringify(request));
    const rejectedBeforeAdmission = reply.status === 409 && reply.json?.code === 'graph_exchange_busy'
      && reply.json.parameters?.reason === 'graph mutation in progress';
    if (!rejectedBeforeAdmission) {
      assert.ok(reply.status >= 200 && reply.status < 300,
        `Recorder control failed: HTTP ${reply.status} ${reply.body}`);
      return reply.json;
    }
    const catalog = await api('graphs');
    const recording = await api(path);
    const observed = { action: request.request.action, expectedRevision: request.expectedRevision,
      graphId: catalog.activeId, revision: catalog.document.revision,
      recording: { state: recording.state, path: recording.path } };
    observe(observed);
    assert.equal(catalog.activeId, graphId, 'External graph selection changed during recorder admission');
    assert.equal(catalog.document.revision, request.expectedRevision, 'External graph revision changed during recorder admission');
    assert.deepEqual(observed.recording, request.expectedRecording, 'Recording changed after a rejected command');
    await wait(250);
  }
  assert.fail('Recorder control remained busy without admitting the command');
}
