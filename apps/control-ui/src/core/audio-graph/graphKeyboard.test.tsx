import type { KeyboardEvent } from 'react';
import { expect, it, vi } from 'vitest';
import { graphContextMenuAnchor, handleGraphKeyDown } from './graphKeyboard';

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

function keyEvent(key: string, target: Element, currentTarget: Element = target) {
  return {
    key,
    target,
    currentTarget,
    repeat: false,
    shiftKey: false,
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    nativeEvent: { isComposing: false },
    preventDefault: vi.fn(),
    stopPropagation: vi.fn(),
  } as unknown as KeyboardEvent<HTMLElement> & {
    preventDefault: ReturnType<typeof vi.fn>;
  };
}

function graphElements() {
  const canvas = document.createElement('section');
  const node = document.createElement('div');
  node.className = 'react-flow__node';
  node.dataset.id = 'mic';
  const control = document.createElement('button');
  node.append(control);
  canvas.append(node);
  return { canvas, node, control };
}

function actions(renameNode?: (nodeId: string) => void) {
  return { capturePortKeys: vi.fn(), clearSelection: vi.fn(), renameNode };
}

it('renames the focused node on F2 without passing the key to port selection', () => {
  const { canvas, node, control } = graphElements();
  const owner = actions(vi.fn());
  const event = keyEvent('F2', node, canvas);
  handleGraphKeyDown(event, owner);
  expect(owner.renameNode).toHaveBeenCalledWith('mic');
  expect(event.preventDefault).toHaveBeenCalled();
  expect(owner.capturePortKeys).not.toHaveBeenCalled();
  // Controls inside the node and a graph without a rename owner keep F2.
  handleGraphKeyDown(keyEvent('F2', control, canvas), owner);
  const unnamed = actions();
  handleGraphKeyDown(keyEvent('F2', node, canvas), unnamed);
  expect(owner.renameNode).toHaveBeenCalledOnce();
  expect(owner.capturePortKeys).toHaveBeenCalledOnce();
  expect(unnamed.capturePortKeys).toHaveBeenCalledOnce();
});

it('clears the selection on Escape from a node or cable, keeping focus in place', () => {
  const { canvas, node, control } = graphElements();
  const owner = actions();
  const event = keyEvent('Escape', node, canvas);
  handleGraphKeyDown(event, owner);
  expect(owner.clearSelection).toHaveBeenCalledOnce();
  expect(event.preventDefault).toHaveBeenCalled();
  handleGraphKeyDown(keyEvent('Escape', control, canvas), owner);
  expect(owner.clearSelection).toHaveBeenCalledOnce();
  expect(owner.capturePortKeys).toHaveBeenCalledTimes(2);
});

it('opens the context menu from the keyboard ahead of every other canvas key', () => {
  const { canvas, node } = graphElements();
  const owner = actions(vi.fn());
  const opened = vi.fn();
  node.addEventListener('contextmenu', opened);
  handleGraphKeyDown(keyEvent('ContextMenu', node, canvas), owner);
  expect(opened).toHaveBeenCalledOnce();
  expect(owner.capturePortKeys).not.toHaveBeenCalled();
});
