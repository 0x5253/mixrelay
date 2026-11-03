import { act, renderHook } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { AudioGraphNode } from './model';
import { useAudioNodeLayout } from './useAudioNodeLayout';

const updateNodeInternals = vi.hoisted(() => vi.fn());
vi.mock('@xyflow/react', () => ({ useUpdateNodeInternals: () => updateNodeInternals }));

it('ignores repeated equal control measurements while responding to a changed disclosure height', () => {
  const node: AudioGraphNode = {
    id: 'input',
    position: { x: 0, y: 0 },
    inputPorts: [],
    outputPorts: [],
    appearance: { category: 'Input', icon: 'MIC', title: 'Input' },
    size: { width: 240, height: 160 },
  };
  const view = renderHook(() => useAudioNodeLayout(node, { width: null, height: null }));
  act(() => view.result.current.onContentHeightChange(70, 40));
  expect(view.result.current.height).toBe(200);
  updateNodeInternals.mockClear();
  act(() => view.result.current.onContentHeightChange(70, 40));
  expect(view.result.current.height).toBe(200);
  expect(updateNodeInternals).not.toHaveBeenCalled();
  act(() => view.result.current.onContentHeightChange(70, 30));
  expect(view.result.current.height).toBe(190);
});
