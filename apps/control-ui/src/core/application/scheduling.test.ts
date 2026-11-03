import { expect, it } from 'vitest';
import { fakeScheduler } from '../../test-support/fakeScheduler';
import { repeat } from './scheduling';

it('repeats on the injected scheduler until cancelled', () => {
  const time = fakeScheduler();
  const observed: number[] = [];
  const cancel = repeat(time, 100, () => observed.push(time.now()));
  time.advance(250);
  expect(observed).toEqual([100, 200]);
  cancel();
  cancel();
  time.advance(1000);
  expect(observed).toEqual([100, 200]);
  expect(time.pending()).toBe(0);
});

it('does not schedule another run when the task cancels its own repetition', () => {
  const time = fakeScheduler();
  const cancel = repeat(time, 100, () => cancel());
  time.advance(100);
  expect(time.pending()).toBe(0);
});
