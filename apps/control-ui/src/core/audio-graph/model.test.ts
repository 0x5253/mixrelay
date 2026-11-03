import { expect, it } from 'vitest';
import { type AudioGraphModel, moveAudioGraphNode } from './model';

const model: AudioGraphModel = {
  revision: 6,
  nodes: [
    {
      id: 'b',
      position: { x: 0, y: 0 },
      inputPorts: [{ id: 'in', label: 'in' }],
      outputPorts: [],
      appearance: { category: 'Test', icon: 'T', title: 'b' },
    },
  ],
  edges: [],
};

it('moves nodes without changing the audio revision', () => {
  expect(moveAudioGraphNode(model, 'missing', { x: 10, y: 20 })).toBe(model);
  const moved = moveAudioGraphNode(model, 'b', { x: 10, y: 20 });
  expect(moved.revision).toBe(model.revision);
  expect(moved.nodes[0]?.position).toEqual({ x: 10, y: 20 });
  expect(moveAudioGraphNode(moved, 'b', { x: 10, y: 20 })).toBe(moved);
});
