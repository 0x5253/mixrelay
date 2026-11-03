import assert from 'node:assert/strict';
import test from 'node:test';
import { finishAfterCleanup } from '../core/qualification-cleanup.mjs';

test('permission observer remains alive while Quit and removal drain native work', async () => {
  const events = [];
  const result = await finishAfterCleanup({ finish: async () => {
    events.push('observer finished');
    return { passed: true };
  } }, async () => {
    events.push('Quit');
    await Promise.resolve();
    events.push('removal');
  });
  assert.deepEqual(events, ['Quit', 'removal', 'observer finished']);
  assert.equal(result.passed, true);
});

test('observer completion still runs after cleanup failure and neither failure becomes a pass', async () => {
  let finished = false;
  await assert.rejects(finishAfterCleanup({ finish: async () => {
    finished = true;
    return { passed: true };
  } }, async () => { throw new Error('Quit failed'); }), /Quit failed/);
  assert.equal(finished, true);
  assert.deepEqual(await finishAfterCleanup({ finish: async () => ({ passed: false, error: 'observer failed' }) }, async () => {}),
    { passed: false, error: 'observer failed' });
});
