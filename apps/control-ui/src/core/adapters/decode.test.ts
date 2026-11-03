import { expect, it } from 'vitest';
import {
  array,
  boolean,
  InvalidResponseError,
  literal,
  nonNegativeInteger,
  nullable,
  number,
  object,
  optional,
  record,
  string,
  tagged,
  unnamedFields,
} from './decode';

const failure = (run: () => unknown) => {
  try {
    run();
  } catch (error) {
    return error instanceof InvalidResponseError ? error.path : 'other error';
  }
  return 'accepted';
};

it('accepts only the JSON value each primitive names', () => {
  expect(string('a', 'x')).toBe('a');
  expect(boolean(false, 'x')).toBe(false);
  expect(number(-0.5, 'x')).toBe(-0.5);
  expect(nonNegativeInteger(0, 'x')).toBe(0);
  for (const [decoder, value] of [
    [string, 1],
    [boolean, 'true'],
    [number, Number.NaN],
    [number, Number.POSITIVE_INFINITY],
    [nonNegativeInteger, 1.5],
    [nonNegativeInteger, -1],
    [nonNegativeInteger, '1'],
    [nonNegativeInteger, 2 ** 53],
  ] as const)
    expect(failure(() => decoder(value, 'value'))).toBe('value');
});

it('checks literals, null and absence separately', () => {
  const state = literal('active', 'pending', 1);
  expect(state('pending', 'x')).toBe('pending');
  expect(state(1, 'x')).toBe(1);
  expect(failure(() => state('done', 'state'))).toBe('state');
  expect(() => state('done', 'state')).toThrow('expected "active" | "pending" | 1');
  expect(nullable(string)(null, 'x')).toBeNull();
  expect(optional(string)(undefined, 'x')).toBeUndefined();
  expect(failure(() => optional(string)(null, 'field'))).toBe('field');
});

it('names the exact path of a nested failure in arrays, records and objects', () => {
  const decoder = object<{ items: { id: string }[]; byName: Record<string, number> }>({
    items: array(object({ id: string })),
    byName: record(number),
  });
  expect(failure(() => decoder({ items: [{ id: 'a' }, { id: 2 }], byName: {} }, 'reply'))).toBe(
    'reply.items[1].id',
  );
  expect(failure(() => decoder({ items: [], byName: { gain: 'x' } }, 'reply'))).toBe(
    'reply.byName.gain',
  );
  expect(failure(() => decoder({ items: {}, byName: {} }, 'reply'))).toBe('reply.items');
  expect(failure(() => decoder({ items: [], byName: [] }, 'reply'))).toBe('reply.byName');
  expect(failure(() => decoder([], 'reply'))).toBe('reply');
  expect(failure(() => decoder(null, 'reply'))).toBe('reply');
});

it('keeps fields the contract does not name and leaves absent optional fields absent', () => {
  const decoder = object<{ id: string; label?: string }>({ id: string, label: optional(string) });
  const decoded = decoder({ id: 'a', added: { by: 'newer engine' } }, 'reply');
  expect(decoded).toEqual({ id: 'a', added: { by: 'newer engine' } });
  expect('label' in decoded).toBe(false);
});

it('selects a tagged variant by its tag and rejects an unknown or missing tag', () => {
  type Mapping = { mode: 'auto' } | { mode: 'explicit'; routes: number[] };
  const mapping = tagged<Mapping>('mode', {
    auto: object({ mode: literal('auto') }),
    explicit: object({ mode: literal('explicit'), routes: array(nonNegativeInteger) }),
  });
  expect(mapping({ mode: 'explicit', routes: [1] }, 'm')).toEqual({
    mode: 'explicit',
    routes: [1],
  });
  expect(failure(() => mapping({ mode: 'manual' }, 'm'))).toBe('m.mode');
  expect(failure(() => mapping({ mode: 7 }, 'm'))).toBe('m.mode');
  expect(failure(() => mapping({ mode: 'toString' }, 'm'))).toBe('m.mode');
  expect(failure(() => mapping('auto', 'm'))).toBe('m.mode');
  expect(failure(() => mapping({ mode: 'explicit', routes: [-1] }, 'm'))).toBe('m.routes[0]');
});

it('lists the fields no contract named, only while asked', async () => {
  const pair = object({ id: string, inner: object({ id: string }) });
  const value = { id: 'a', extra: 1, inner: { id: 'b', renamed: 2 } };
  await expect(unnamedFields(() => pair(value, 'p'))).resolves.toEqual([
    'p.extra',
    'p.inner.renamed',
  ]);
  await expect(unnamedFields(() => pair({ id: 1 }, 'p'))).rejects.toThrow(InvalidResponseError);
  // A failed run stops collecting, so ordinary decoding never pays for it.
  expect(pair(value, 'p')).toEqual(value);
  await expect(unnamedFields(() => undefined)).resolves.toEqual([]);
});
