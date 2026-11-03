import { expect, it } from 'vitest';
import { sameViewport, viewportToRestore, viewportToSave } from './graphViewport';

const saved = { x: 10, y: -4, zoom: 0.5 };

it('compares every viewport component', () => {
  expect(sameViewport(saved, { ...saved })).toBe(true);
  for (const change of [{ x: 11 }, { y: 0 }, { zoom: 1 }])
    expect(sameViewport(saved, { ...saved, ...change })).toBe(false);
});

it('restores a requested view only when the canvas shows a different one', () => {
  expect(viewportToRestore({ x: 0, y: 0, zoom: 1 }, saved)).toEqual(saved);
  expect(viewportToRestore({ x: 0, y: 0, zoom: 1 }, saved)).not.toBe(saved);
  expect(viewportToRestore({ ...saved }, saved)).toBeUndefined();
  expect(viewportToRestore(saved, undefined)).toBeUndefined();
});

it('saves every move except the echo of the requested view', () => {
  const moved = { x: 3, y: 4, zoom: 2 };
  expect(viewportToSave(saved, moved)).toEqual(moved);
  expect(viewportToSave(undefined, moved)).toEqual(moved);
  expect(viewportToSave(saved, { ...saved })).toBeUndefined();
});
