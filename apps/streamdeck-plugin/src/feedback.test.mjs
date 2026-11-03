import assert from 'node:assert/strict';
import test from 'node:test';
import { Feedback } from './feedback.mjs';

test('rapid display updates render only the latest state at 100 ms intervals', async () => {
  const timers = [], rendered = [];
  const feedback = new Feedback(async entry => rendered.push(entry.value), (callback, delay) => {
    assert.equal(delay, 100);
    timers.push(callback);
    return timers.length;
  });
  const entry = value => ({ action: { id: 'key' }, value });
  for (let value = 0; value < 100; value++) feedback.update(entry(value));
  assert.equal(timers.length, 1);
  assert.deepEqual(rendered, []);
  await timers.shift()();
  assert.deepEqual(rendered, [99]);
  feedback.update(entry(100));
  assert.equal(timers.length, 1);
  await timers.shift()();
  assert.deepEqual(rendered, [99, 100]);
});

test('a slow or failed frame cannot overlap the next frame or keep a disappeared action queued', async () => {
  const timers = [], rendered = [];
  const held = Promise.withResolvers();
  const feedback = new Feedback(async entry => {
    rendered.push(entry.value);
    await held.promise;
    throw new Error('display disconnected');
  }, callback => { timers.push(callback); return timers.length; });
  feedback.update({ action: { id: 'dial' }, value: 1 });
  const rendering = timers.shift()();
  feedback.update({ action: { id: 'dial' }, value: 2 });
  feedback.update({ action: { id: 'removed' }, value: 3 });
  feedback.remove('removed');
  assert.equal(timers.length, 0);
  held.resolve();
  await rendering;
  assert.equal(timers.length, 1);
  await timers.shift()();
  assert.deepEqual(rendered, [1, 2]);
});
