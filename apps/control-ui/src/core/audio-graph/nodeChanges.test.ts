import { expect, it } from 'vitest';
import type { AudioGraphModel } from './model';
import { applyNodeLayoutChanges, resizedNodeSizes } from './nodeChanges';

const model: AudioGraphModel = {
  revision: 3,
  nodes: [
    {
      id: 'gain',
      position: { x: 0, y: 0 },
      inputPorts: [],
      outputPorts: [],
      appearance: { category: 'Test', icon: 'T', title: 'gain' },
    },
  ],
  edges: [],
};

it('keeps only user resizes, without the temporary Options height', () => {
  const sizes = resizedNodeSizes(
    [
      {
        type: 'dimensions',
        id: 'gain',
        dimensions: { width: 300, height: 260 },
        setAttributes: true,
      },
      { type: 'dimensions', id: 'measured', dimensions: { width: 1, height: 1 } },
      { type: 'dimensions', id: 'empty', setAttributes: true },
      { type: 'position', id: 'gain' },
    ],
    new Map([['gain', 60]]),
  );
  expect([...sizes]).toEqual([['gain', { width: 300, height: 200 }]]);
});

it('applies moves and user resizes to the layout without a new audio revision', () => {
  const next = applyNodeLayoutChanges(
    model,
    [
      { type: 'position', id: 'gain', position: { x: 24, y: 48 } },
      { type: 'position', id: 'gain' },
      {
        type: 'dimensions',
        id: 'gain',
        dimensions: { width: 320, height: 140 },
        setAttributes: 'width',
      },
      { type: 'dimensions', id: 'gain', dimensions: { width: 1, height: 1 } },
      { type: 'select', id: 'gain' },
    ],
    new Map(),
  );
  expect(next.revision).toBe(3);
  expect(next.nodes[0]).toMatchObject({
    position: { x: 24, y: 48 },
    size: { width: 320, height: 140 },
  });
  expect(applyNodeLayoutChanges(model, [{ type: 'remove', id: 'gain' }], new Map())).toBe(model);
});
