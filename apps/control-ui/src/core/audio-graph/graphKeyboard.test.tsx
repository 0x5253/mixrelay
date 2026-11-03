import { expect, it, vi } from 'vitest';
import { graphContextMenuAnchor } from './graphKeyboard';

it('anchors a coordinate-less AXShowMenu to the node even when the target is its title', () => {
  const node = document.createElement('div');
  node.className = 'react-flow__node';
  const title = document.createElement('span');
  node.append(title);
  vi.spyOn(node, 'getBoundingClientRect').mockReturnValue(new DOMRect(402, 255, 420, 262));
  expect(graphContextMenuAnchor({ target: title, clientX: 0, clientY: 0 })).toEqual({
    x: 612,
    y: 386,
  });
});

it('keeps real pointer coordinates and keyboard-generated owner coordinates', () => {
  expect(graphContextMenuAnchor({ target: null, clientX: 507, clientY: 270 })).toEqual({
    x: 507,
    y: 270,
  });
});

it('anchors a coordinate-less canvas invocation to its surface and tolerates missing targets', () => {
  const canvas = document.createElement('section');
  vi.spyOn(canvas, 'getBoundingClientRect').mockReturnValue(new DOMRect(200, 64, 824, 580));
  expect(graphContextMenuAnchor({ target: canvas, clientX: 0, clientY: 0 })).toEqual({
    x: 612,
    y: 354,
  });
  expect(graphContextMenuAnchor({ target: null, clientX: 0, clientY: 0 })).toEqual({ x: 0, y: 0 });
});
