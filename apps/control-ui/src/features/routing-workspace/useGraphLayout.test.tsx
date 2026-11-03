import { act, renderHook, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { GraphLayoutResponse } from '../../core/domain/control/catalog';
import { catalogOf, graphWithNodes } from '../../test-support/portDefaults';

/** Every node the layouts in this file place. */
const layoutGraph = graphWithNodes(['a', 'b', 'concurrent', 'fresh', 'input', 'output']);

import type { LayoutPort } from '../../core/application/workspace/GraphLayoutController';
import { useGraphLayout } from './useGraphLayout';

function model(): AudioGraphModel {
  return {
    revision: 1,
    edges: [],
    nodes: [
      {
        id: 'virtual:input',
        appearance: { title: 'Input', category: 'Source', icon: 'IN' },
        position: { x: 0, y: 0 },
        inputPorts: [],
        outputPorts: [],
      },
      {
        id: 'virtual:output',
        appearance: { title: 'Output', category: 'Output', icon: 'OUT' },
        position: { x: 0, y: 0 },
        inputPorts: [],
        outputPorts: [],
      },
    ],
  };
}
const response: GraphLayoutResponse = {
  graphId: 'a',
  layoutRevision: 4,
  layout: {
    positions: { 'virtual:input': { x: 100, y: 120 }, 'virtual:output': { x: 900, y: -50 } },
    sizes: { 'virtual:input': { width: 360, height: 220 } },
    viewport: { x: -200, y: 75, zoom: 0.6 },
  },
};
afterEach(() => vi.useRealTimers());

describe('workspace layout restoration', () => {
  it('loads after the audio model arrives without rewriting saved positions or audio revision', async () => {
    const getGraphLayout = vi.fn().mockResolvedValue(response);
    const setGraphLayout = vi.fn();
    const port: LayoutPort = {
      getGraphLayout,
      setGraphLayout,
      getGraphCatalog: vi.fn(async () => catalogOf('a', layoutGraph)),
    };
    const hook = renderHook(() => {
      const [graph, setGraph] = useState<AudioGraphModel | null>(null);
      return { graph, setGraph, layout: useGraphLayout(port, 'a', graph, setGraph) };
    });
    await waitFor(() => expect(hook.result.current.layout.loaded).toBe(true));
    act(() => hook.result.current.setGraph(model()));
    await waitFor(() => expect(hook.result.current.graph?.nodes[1]?.position.x).toBe(900));
    expect(hook.result.current.graph?.nodes[0]?.position).toEqual({ x: 100, y: 120 });
    expect(hook.result.current.graph?.nodes[0]?.size).toEqual({ width: 360, height: 220 });
    expect(hook.result.current.graph?.revision).toBe(1);
    expect(hook.result.current.layout.layout.viewport).toEqual(response.layout.viewport);
    expect(setGraphLayout).not.toHaveBeenCalled();
    hook.unmount();
  });

  it('checkpoints explicit position/Undo and viewport changes, preserving split endpoint IDs', async () => {
    vi.useFakeTimers();
    const getGraphLayout = vi.fn().mockResolvedValue(response);
    const setGraphLayout = vi.fn().mockResolvedValue({ ...response, layoutRevision: 5 });
    const port: LayoutPort = {
      getGraphLayout,
      setGraphLayout,
      getGraphCatalog: vi.fn(async () => catalogOf('a', layoutGraph)),
    };
    const hook = renderHook(() => {
      const [graph, setGraph] = useState<AudioGraphModel | null>(model());
      return useGraphLayout(port, 'a', graph, setGraph);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    act(() => {
      const resized = model();
      resized.nodes = resized.nodes.map((node, index) =>
        index === 0 ? { ...node, size: { width: 400, height: 260 } } : node,
      );
      hook.result.current.updateNodes(resized);
      hook.result.current.updateViewport({ x: 30, y: 40, zoom: 1.2 });
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(400);
    });
    expect(setGraphLayout).toHaveBeenCalledExactlyOnceWith('a', {
      expectedLayoutRevision: 4,
      layout: {
        positions: { 'virtual:input': { x: 0, y: 0 }, 'virtual:output': { x: 0, y: 0 } },
        sizes: { 'virtual:input': { width: 400, height: 260 } },
        viewport: { x: 30, y: 40, zoom: 1.2 },
      },
    });
    hook.unmount();
  });
});

it('clamps saved EQ widths below the Core minimum without changing saved audio or writing layout', async () => {
  const saved = {
    ...response,
    layout: { ...response.layout, sizes: { 'virtual:input': { width: 228, height: 160 } } },
  };
  const getGraphLayout = vi.fn().mockResolvedValue(saved);
  const setGraphLayout = vi.fn();
  const port: LayoutPort = {
    getGraphLayout,
    setGraphLayout,
    getGraphCatalog: vi.fn(async () => catalogOf('a', layoutGraph)),
  };
  const initial = model();
  const eq = initial.nodes[0];
  if (!eq) throw new Error('Expected node');
  eq.appearance.equalizer = { bands: [] };
  const hook = renderHook(() => {
    const [graph, setGraph] = useState<AudioGraphModel | null>(initial);
    return { graph, layout: useGraphLayout(port, 'a', graph, setGraph) };
  });
  await waitFor(() => expect(hook.result.current.graph?.nodes[0]?.size?.width).toBe(320));
  expect(hook.result.current.graph?.nodes[0]?.size?.height).toBe(160);
  expect(hook.result.current.graph?.nodes[0]?.position).toEqual({ x: 100, y: 120 });
  expect(hook.result.current.graph?.revision).toBe(initial.revision);
  expect(saved.layout.sizes['virtual:input'].width).toBe(228);
  expect(setGraphLayout).not.toHaveBeenCalled();
  hook.unmount();
});

it('checkpoints labels per UI identity, resets a label and prunes only removed instances', async () => {
  const read = vi.fn().mockResolvedValue(response);
  const write = vi.fn().mockImplementation(async (_id, request) => ({
    graphId: 'a',
    layoutRevision: 5,
    layout: request.layout,
  }));
  const port: LayoutPort = {
    getGraphLayout: read,
    setGraphLayout: write,
    getGraphCatalog: vi.fn(async () => catalogOf('a', layoutGraph)),
  };
  const hook = renderHook(() => {
    const [graph, setGraph] = useState<AudioGraphModel | null>(model());
    return useGraphLayout(port, 'a', graph, setGraph);
  });
  await waitFor(() => expect(hook.result.current.loaded).toBe(true));
  act(() => hook.result.current.updateNodeLabel('virtual:input', '  Video  '));
  act(() => hook.result.current.updateNodeLabel('virtual:output', 'Meeting'));
  expect(hook.result.current.layout.nodeLabels).toEqual({
    'virtual:input': 'Video',
    'virtual:output': 'Meeting',
  });
  act(() => hook.result.current.updateNodes(model()));
  expect(hook.result.current.layout.nodeLabels).toEqual({
    'virtual:input': 'Video',
    'virtual:output': 'Meeting',
  });
  act(() => hook.result.current.updateNodeLabel('virtual:input', '  '));
  expect(hook.result.current.layout.nodeLabels).toEqual({ 'virtual:output': 'Meeting' });
  act(() =>
    hook.result.current.updateNodes({
      ...model(),
      nodes: model().nodes.filter((node) => node.id === 'virtual:input'),
    }),
  );
  expect(hook.result.current.layout.nodeLabels).toEqual({});
  act(() => hook.result.current.updateNodeLabel('__proto__', 'Video'));
  act(() => hook.result.current.updateNodeLabel('constructor', 'Meeting'));
  expect(Object.entries(hook.result.current.layout.nodeLabels ?? {})).toEqual([
    ['__proto__', 'Video'],
    ['constructor', 'Meeting'],
  ]);
  act(() => hook.result.current.updateNodeLabel('__proto__', ''));
  act(() => hook.result.current.updateNodeLabel('constructor', ''));
  await waitFor(() => expect(write).toHaveBeenCalled());
  expect(write).toHaveBeenCalledWith(
    'a',
    expect.objectContaining({ layout: expect.objectContaining({ nodeLabels: {} }) }),
  );
  hook.unmount();
});
