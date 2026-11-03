import type { ReactFlowInstance } from '@xyflow/react';
import { type RefObject, useEffect, useRef } from 'react';
import type { AudioGraphViewport } from './graphViewport';
import { graphNodeDefaultWidth, normalizeAudioGraphNodeSize } from './model';
import {
  defaultInsertionSize,
  freeNodePosition,
  type PlaceGraphNodes,
  viewportRevealingNode,
} from './nodePlacement';
import type { AudioFlowEdge, AudioFlowNode } from './reactFlowAdapter';

interface InsertionSurface {
  flowRef: RefObject<Pick<
    ReactFlowInstance<AudioFlowNode, AudioFlowEdge>,
    'getNodes' | 'getViewport' | 'setViewport' | 'screenToFlowPosition'
  > | null>;
  rootRef: RefObject<HTMLElement | null>;
}

/** Core owns rendered bounds, graph coordinates and revealing insertions. Feature
 * applies the returned layout to its insertion in the same document update. */
export function useNodeInsertion(
  surface: InsertionSurface,
  onReveal?: (viewport: AudioGraphViewport) => void,
) {
  const pending = useRef(new Set<string>());
  const placeNodes: PlaceGraphNodes = (next) => {
    const flow = surface.flowRef.current;
    const root = surface.rootRef.current;
    if (!flow || !root) return next;
    const bounds = root.getBoundingClientRect();
    const origin = flow.screenToFlowPosition({ x: bounds.left + 16, y: bounds.top + 48 });
    const end = flow.screenToFlowPosition({ x: bounds.right - 16, y: bounds.bottom - 16 });
    const viewport = { ...origin, width: end.x - origin.x, height: end.y - origin.y };
    const current = flow.getNodes();
    const existing = new Set(current.map((node) => node.id));
    const occupied = current.map((node) => ({
      ...node.position,
      width: node.measured?.width ?? node.width ?? defaultInsertionSize.width,
      height: node.measured?.height ?? node.height ?? defaultInsertionSize.height,
    }));
    const placed = next.nodes.map((node) => {
      if (existing.has(node.id)) return node;
      const width =
        normalizeAudioGraphNodeSize(node, node.size)?.width ?? graphNodeDefaultWidth(node);
      const position = freeNodePosition(node.position, width, occupied, viewport);
      // Batch insertions keep distinct strips until all their content has been measured.
      occupied.push({ ...position, width, height: Number.POSITIVE_INFINITY });
      pending.current.add(node.id);
      return { ...node, position };
    });
    return { ...next, nodes: placed };
  };
  useEffect(() => {
    const flow = surface.flowRef.current;
    const root = surface.rootRef.current;
    if (!flow || !root || !pending.current.size) return;
    // React Flow publishes measured dimensions through its node changes.
    for (const node of flow.getNodes()) {
      if (!pending.current.has(node.id) || !node.measured?.width || !node.measured.height) continue;
      const viewport = flow.getViewport();
      const next = viewportRevealingNode(
        {
          ...node.position,
          ...node.measured,
          width: node.measured.width,
          height: node.measured.height,
        },
        viewport,
        root.getBoundingClientRect(),
      );
      if (next.x !== viewport.x || next.y !== viewport.y || next.zoom !== viewport.zoom) {
        void flow.setViewport(next);
        onReveal?.(next);
      }
      pending.current.delete(node.id);
    }
  });
  return placeNodes;
}
