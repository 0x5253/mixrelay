import { expect, it } from 'vitest';
import { fakeScheduler } from './fakeScheduler';

it('runs due tasks in time order, then scheduling order, including tasks they schedule', () => {
  const scheduler = fakeScheduler();
  const order: string[] = [];
  scheduler.after(20, () => order.push('b@20'));
  scheduler.after(10, () => {
    order.push(`a@${scheduler.now()}`);
    scheduler.after(5, () => order.push(`nested@${scheduler.now()}`));
  });
  scheduler.after(20, () => order.push('c@20'));
  const cancel = scheduler.after(15, () => order.push('cancelled'));
  cancel();
  scheduler.after(-5, () => order.push('now@0'));
  scheduler.advance(19);
  expect(order).toEqual(['now@0', 'a@10', 'nested@15']);
  expect(scheduler.now()).toBe(19);
  expect(scheduler.pending()).toBe(2);
  scheduler.advance(1);
  expect(order.slice(3)).toEqual(['b@20', 'c@20']);
  expect(scheduler.pending()).toBe(0);
});
