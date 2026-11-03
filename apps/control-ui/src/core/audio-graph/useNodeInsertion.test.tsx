import { renderHook } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { AudioGraphModel, AudioGraphNode } from './model';
import { reconcileAudioFlowNodes } from './reactFlowAdapter';
import { useNodeInsertion } from './useNodeInsertion';

function graphNode(id: string): AudioGraphNode {
  return {
    id,
    position: { x: 132, y: 92 },
    inputPorts: [],
    outputPorts: [],
    appearance: { category: 'Output', icon: 'REC', title: id },
  };
}
function model(nodes: AudioGraphNode[]): AudioGraphModel {
  return { nodes, edges: [], revision: 1 };
}
function surface() {
  const root = document.createElement('section');
  vi.spyOn(root, 'getBoundingClientRect').mockReturnValue(new DOMRect(200, 64, 824, 580));
  let nodes = reconcileAudioFlowNodes([], model([graphNode('compressor')]));
  nodes = nodes.map((node) => ({
    ...node,
    position: { x: 200, y: 160 },
    measured: { width: 420, height: 262 },
  }));
  const viewport = { x: -300, y: 40, zoom: 0.8 };
  const flow = {
    getNodes: () => nodes,
    getViewport: () => viewport,
    setViewport: vi.fn(async (_viewport: { x: number; y: number; zoom: number }) => true),
    screenToFlowPosition: ({ x, y }: { x: number; y: number }) => ({
      x: (x - 200 - viewport.x) / viewport.zoom,
      y: (y - 64 - viewport.y) / viewport.zoom,
    }),
  };
  return {
    rootRef: { current: root },
    flowRef: { current: flow },
    flow,
    replace: (next: typeof nodes) => {
      nodes = next;
    },
  };
}

it('uses measured occupied cards and reveals only inserted nodes without moving focus', () => {
  const s = surface();
  const focus = document.createElement('button');
  document.body.append(focus);
  focus.focus();
  const onReveal = vi.fn();
  const { result, rerender } = renderHook(() => useNodeInsertion(s, onReveal));
  const next = model([graphNode('compressor'), graphNode('recorder')]);
  const placed = result.current(next);
  expect(placed.nodes[0]).toBe(next.nodes[0]);
  const recorder = placed.nodes[1];
  if (!recorder) throw new Error('Expected inserted recorder');
  expect(
    recorder.position.y >= 454 || recorder.position.x >= 652 || recorder.position.x + 228 <= 168,
  ).toBe(true);
  expect(document.activeElement).toBe(focus);
  const rendered = reconcileAudioFlowNodes([], placed);
  s.replace(rendered); // Measurements arrive after the model publication.
  rerender();
  expect(s.flow.setViewport).not.toHaveBeenCalled();
  s.replace(
    rendered.map((node) => ({
      ...node,
      position: { x: 2000, y: 1000 },
      measured: { width: 420, height: 800 },
    })),
  );
  rerender();
  expect(s.flow.setViewport).toHaveBeenCalledTimes(1);
  expect(onReveal).toHaveBeenCalledWith(s.flow.setViewport.mock.calls[0]?.[0]);
  rerender();
  expect(s.flow.setViewport).toHaveBeenCalledTimes(1);
  expect(document.activeElement).toBe(focus);
  focus.remove();
});

it('reserves separate strips in a batch and preserves an already visible viewport', () => {
  const s = surface();
  s.replace([]);
  const { result, rerender } = renderHook(() => useNodeInsertion(s));
  const a = { ...graphNode('a'), size: { width: 300, height: 120 }, position: { x: 420, y: 80 } };
  const placed = result.current(model([a, graphNode('b')]));
  const [first, second] = placed.nodes;
  if (!first || !second) throw new Error('Expected two insertions');
  expect(Math.abs(first.position.x - second.position.x)).toBeGreaterThanOrEqual(260);
  s.replace(
    reconcileAudioFlowNodes([], model([first])).map((node) => ({
      ...node,
      measured: { width: 300, height: 120 },
    })),
  );
  rerender();
  expect(s.flow.setViewport).not.toHaveBeenCalled();
});

it('handles saved dimensions and unmeasured cards without replacing existing nodes', () => {
  const s = surface();
  const previous = graphNode('old');
  s.replace(
    reconcileAudioFlowNodes([], model([previous])).map((node) => ({
      ...node,
      width: 300,
      height: 400,
    })),
  );
  const { result } = renderHook(() => useNodeInsertion(s));
  expect(result.current(model([previous, graphNode('new')])).nodes[0]).toBe(previous);
  s.replace(reconcileAudioFlowNodes([], model([previous])));
  expect(result.current(model([previous, graphNode('next')])).nodes[1]?.position).not.toEqual(
    previous.position,
  );
});

it('keeps the model intact before the canvas and React Flow mount', () => {
  const next = model([graphNode('new')]);
  const { result } = renderHook(() =>
    useNodeInsertion({ rootRef: { current: null }, flowRef: { current: null } }),
  );
  expect(result.current(next)).toBe(next);
});
