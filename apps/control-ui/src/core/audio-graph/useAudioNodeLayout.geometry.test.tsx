import { act, renderHook } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { audioNodePortTop } from '../ui/audioNodeLayout';
import type { AudioGraphNode } from './model';
import { graphNodeFaceGeometry, useAudioNodeLayout } from './useAudioNodeLayout';

const updateNodeInternals = vi.fn();
vi.mock('@xyflow/react', () => ({
  useUpdateNodeInternals: () => updateNodeInternals,
}));

function node(type: 'audio_input' | 'recorder' | 'provider'): AudioGraphNode {
  return {
    id: 'node',
    position: { x: 0, y: 0 },
    runtime: { type, config: {} },
    appearance: {
      title: 'Node',
      category: 'Audio',
      icon: 'N',
      meterBanks: [{ channels: 2, nodeId: 'node', portId: 'out' }],
    },
    inputPorts: [{ id: 'in', label: 'Input' }],
    outputPorts: [{ id: 'out', label: 'Output' }],
  };
}

it.each(['audio_input', 'recorder', 'provider'] as const)(
  '%s fits intrinsic controls and borders without a separate resize footer',
  (type) => {
    const graphNode = node(type);
    const face = graphNodeFaceGeometry(graphNode);
    const { result } = renderHook(() =>
      useAudioNodeLayout(graphNode, { width: null, height: null }),
    );
    act(() => result.current.onContentHeightChange(200));
    expect(result.current.minHeight).toBe(face.height + 200 + 2);
    act(() => result.current.onContentHeightChange(24));
    expect(result.current.minHeight).toBe(
      Math.max(type === 'provider' ? 120 : 0, face.height + 26),
    );
  },
);

it('keeps explicit plugin height without adding a footer to its port geometry', () => {
  const graphNode = { ...node('provider'), size: { width: 400, height: 350 } };
  const { result } = renderHook(() => useAudioNodeLayout(graphNode, { width: 400, height: 200 }));
  expect(result.current.ports.height).toBe(350);
  expect(graphNode.size).toEqual({ width: 400, height: 350 });
});

it('fits native editor geometry with only the header and card borders', () => {
  const { result } = renderHook(() =>
    useAudioNodeLayout(node('provider'), { width: 400, height: 200 }),
  );
  expect(result.current.ports.height).toBe(228);
});

it('keeps compact provider anchors fixed when Options expand and collapse', () => {
  const graphNode = node('provider');
  const { result } = renderHook(() => useAudioNodeLayout(graphNode, { width: null, height: null }));
  act(() => result.current.onContentHeightChange(24));
  const portTop = audioNodePortTop(result.current.ports, 0, 1);
  for (const contentHeight of [200, 420, 24]) {
    act(() => result.current.onContentHeightChange(contentHeight));
    expect(result.current.minHeight).toBe(Math.max(120, 30 + contentHeight));
    expect(result.current.ports.height).toBe(120);
    expect(audioNodePortTop(result.current.ports, 0, 1)).toBe(portTop);
  }
});

it('retains native editor anchors when Options exceed the editor height', () => {
  const { result } = renderHook(() =>
    useAudioNodeLayout(node('provider'), { width: 400, height: 200 }),
  );
  const portTop = audioNodePortTop(result.current.ports, 0, 1);
  for (const contentHeight of [600, 24]) {
    act(() => result.current.onContentHeightChange(contentHeight));
    expect(result.current.ports.height).toBe(228);
    expect(audioNodePortTop(result.current.ports, 0, 1)).toBe(portTop);
  }
});

it('moves compact provider anchors only with an explicit resize, not Options height', () => {
  const graphNode = { ...node('provider'), size: { width: 400, height: 350 } };
  const { result, rerender } = renderHook(
    ({ height }) =>
      useAudioNodeLayout(
        { ...graphNode, size: { ...graphNode.size, height } },
        { width: 400, height: 200 },
      ),
    { initialProps: { height: 350 } },
  );
  const originalTop = audioNodePortTop(result.current.ports, 0, 1);
  act(() => result.current.onContentHeightChange(600));
  expect(result.current.ports.height).toBe(350);
  expect(audioNodePortTop(result.current.ports, 0, 1)).toBe(originalTop);
  rerender({ height: 500 });
  const resizedTop = audioNodePortTop(result.current.ports, 0, 1);
  expect(resizedTop).toBeGreaterThan(originalTop);
  act(() => result.current.onContentHeightChange(24));
  expect(audioNodePortTop(result.current.ports, 0, 1)).toBe(resizedTop);
  rerender({ height: 200 });
  expect(audioNodePortTop(result.current.ports, 0, 1)).toBeLessThan(originalTop);
});

it('keeps compact disclosure temporary while explicit resize changes only its saved baseline', () => {
  const graphNode = node('provider');
  const { result, rerender } = renderHook(
    ({ height }) =>
      useAudioNodeLayout(
        { ...graphNode, size: { width: 300, height } },
        { width: null, height: null },
      ),
    { initialProps: { height: 120 } },
  );
  act(() => result.current.onContentHeightChange(24, 0));
  const baselineTop = audioNodePortTop(result.current.ports, 0, 1);
  act(() => result.current.onContentHeightChange(224, 200));
  expect(result.current.height).toBe(320);
  expect(result.current.disclosureHeight).toBe(200);
  expect(audioNodePortTop(result.current.ports, 0, 1)).toBe(baselineTop);
  rerender({ height: 200 });
  expect(result.current.height).toBe(400);
  expect(result.current.ports.height).toBe(200);
  act(() => result.current.onContentHeightChange(24, 0));
  expect(result.current.height).toBe(200);
  expect(result.current.disclosureHeight).toBe(0);
  expect(result.current.ports.height).toBe(200);
});

it.each(['audio_input', 'recorder'] as const)(
  '%s keeps a stored face and anchors while Options add temporary height',
  (type) => {
    const graphNode = { ...node(type), size: { width: 300, height: 400 } };
    const { result } = renderHook(() =>
      useAudioNodeLayout(graphNode, { width: null, height: null }),
    );
    act(() => result.current.onContentHeightChange(25, 0));
    const closedTop = audioNodePortTop(result.current.ports, 0, 1);
    expect(result.current.faceHeight).toBe(373);
    const naturalMinHeight = graphNodeFaceGeometry(graphNode).height + 25 + 2;
    expect(result.current.minHeight).toBe(naturalMinHeight);
    expect(result.current.minHeight).toBeLessThan(graphNode.size.height);
    expect(result.current.height).toBe(400);

    act(() => result.current.onContentHeightChange(62, 37));
    expect(result.current.minHeight).toBe(naturalMinHeight + 37);
    expect(result.current.height).toBe(437);
    expect(result.current.faceHeight).toBe(373);
    expect(audioNodePortTop(result.current.ports, 0, 1)).toBe(closedTop);

    for (const [total, disclosure] of [
      [85, 60],
      [55, 30],
      [62, 37],
    ] as const) {
      act(() => result.current.onContentHeightChange(total, disclosure));
      expect(result.current.faceHeight).toBe(373);
      expect(audioNodePortTop(result.current.ports, 0, 1)).toBe(closedTop);
    }

    act(() => result.current.onContentHeightChange(25, 0));
    expect(result.current.minHeight).toBe(naturalMinHeight);
    expect(result.current.height).toBe(400);
    expect(audioNodePortTop(result.current.ports, 0, 1)).toBe(closedTop);
    expect(result.current.faceHeight).toBe(373);
  },
);

it.each(['audio_input', 'recorder'] as const)(
  '%s keeps natural face geometry when disclosure opens',
  (type) => {
    const { result } = renderHook(() =>
      useAudioNodeLayout(node(type), { width: null, height: null }),
    );
    act(() => result.current.onContentHeightChange(25, 0));
    const closedTop = audioNodePortTop(result.current.ports, 0, 1);
    const closedMinHeight = result.current.minHeight;
    expect(result.current.height).toBeUndefined();
    act(() => result.current.onContentHeightChange(62, 37));
    expect(result.current.minHeight).toBe(closedMinHeight + 37);
    expect(result.current.height).toBeUndefined();
    expect(audioNodePortTop(result.current.ports, 0, 1)).toBe(closedTop);
  },
);
