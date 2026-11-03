import assert from 'node:assert/strict';

/** Restores only the exact publication owned by this run, after finalizing its recorder. */
export async function restoreQualificationGraph({ unconfirmedMutation, applied, original, graphId, recorderPath }, { api, controlTake, apply }) {
  assert.equal(unconfirmedMutation, false, 'Unconfirmed graph publication; reset the QA session');
  if (!applied) return;
  const current = await api('graphs');
  assert.equal(current.activeId, graphId, 'External selection changed; preserve it');
  assert.deepEqual(current.document, applied, 'External graph change; preserve it');
  if (recorderPath) {
    const take = await api(recorderPath);
    if (['recording', 'paused'].includes(take.state))
      await controlTake({
        expectedRevision: applied.revision,
        expectedRecording: { state: take.state, path: take.path },
        request: { action: 'stop' },
      });
  }
  await apply({ ...original, expectedGraphId: graphId, revision: applied.revision + 1 });
}

/** Keeps UI observation alive until cleanup has been attempted, including failed cleanup. */
export async function finishAfterCleanup(ui, cleanup) {
  let result;
  try {
    await cleanup();
  } finally {
    result = await ui?.finish();
  }
  return result;
}
