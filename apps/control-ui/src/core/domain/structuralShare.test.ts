import { expect, it } from 'vitest';
import { structuralShare } from './structuralShare';

it('returns the previous value when the next one is deep-equal', () => {
  const previous = { a: [1, { b: 'x' }], c: null };
  expect(structuralShare(previous, { a: [1, { b: 'x' }], c: null })).toBe(previous);
});

it('keeps unchanged subtrees and replaces only what changed', () => {
  const kept = { title: 'Mic', ports: [{ id: 'out' }] };
  const changed = { title: 'Out', status: 'ok' };
  const previous = { nodes: [kept, changed], revision: 1 };
  const next = structuralShare(previous, {
    nodes: [
      { title: 'Mic', ports: [{ id: 'out' }] },
      { title: 'Out', status: 'warning' },
    ],
    revision: 2,
  });
  expect(next).not.toBe(previous);
  expect(next.nodes[0]).toBe(kept);
  expect(next.nodes[1]).not.toBe(changed);
  expect(next.nodes[1]).toEqual({ title: 'Out', status: 'warning' });
});

it('treats added, removed and reordered entries as changes', () => {
  const previous = { a: 1, list: [1, 2] };
  expect(structuralShare(previous, { a: 1, b: undefined, list: [1, 2] })).not.toBe(previous);
  expect(structuralShare(previous, { list: [1, 2] })).not.toBe(previous);
  expect(structuralShare(previous, { a: 1, list: [2, 1] }).list).toEqual([2, 1]);
  expect(structuralShare([1, 2], [1])).toEqual([1]);
});

it('compares functions and class instances by identity only', () => {
  const run = () => 1;
  const previous = { run, date: new Date(0) };
  expect(structuralShare(previous, { run, date: previous.date })).toBe(previous);
  const next = structuralShare(previous, { run, date: new Date(0) });
  expect(next).not.toBe(previous);
  expect(next.run).toBe(run);
  expect(next.date).not.toBe(previous.date);
  expect(structuralShare(previous, { ...previous, run: () => 1 }).run).not.toBe(run);
  expect(structuralShare(undefined, previous)).toBe(previous);
  expect(structuralShare({ a: 1 }, [1])).toEqual([1]);
  expect(structuralShare([1], { a: 1 })).toEqual({ a: 1 });
});

it('shares Map entries by value', () => {
  const entry = { state: 'open' };
  const previous = new Map([['a', entry]]);
  expect(structuralShare(previous, new Map([['a', { state: 'open' }]]))).toBe(previous);
  const changed = structuralShare(
    previous,
    new Map([
      ['a', { state: 'open' }],
      ['b', entry],
    ]),
  );
  expect(changed).not.toBe(previous);
  expect(changed.get('a')).toBe(entry);
  expect(structuralShare(previous, new Map([['b', { state: 'open' }]]))).not.toBe(previous);
  expect(structuralShare({ a: 1 }, new Map())).toEqual(new Map());
});

it('treats holes, signed zero and NaN by identity of each index', () => {
  // biome-ignore lint/suspicious/noSparseArray: a hole must not equal a present value.
  const sparse = [, 1];
  expect(structuralShare([1, 1], sparse)).not.toEqual([1, 1]);
  expect(1 in structuralShare([undefined, 1], sparse)).toBe(true);
  expect(0 in structuralShare([undefined, 1], sparse)).toBe(false);
  expect(structuralShare([1], new Array(1))).toHaveLength(1);
  expect(0 in structuralShare([1], new Array(1))).toBe(false);
  const zero = { value: 0 };
  expect(structuralShare(zero, { value: -0 })).not.toBe(zero);
  const missing = { value: Number.NaN };
  expect(structuralShare(missing, { value: Number.NaN })).toBe(missing);
  const sparsePrevious = new Array(2);
  expect(structuralShare(sparsePrevious, new Array(2))).toBe(sparsePrevious);
});
