import { nodeInsertionGap, nodeWidths } from '../theme/tokens';
import type { AudioGraphViewport } from './graphViewport';
import type { AudioGraphModel, AudioGraphPosition } from './model';

export interface GraphRectangle extends AudioGraphPosition {
  width: number;
  height: number;
}

export type PlaceGraphNodes = (model: AudioGraphModel) => AudioGraphModel;

/** A vertical strip permits content to grow before its first measurement without
 * covering another card. Prefer a visible strip nearest the requested position. */
export function freeNodePosition(
  intended: AudioGraphPosition,
  width: number,
  occupied: readonly GraphRectangle[],
  viewport: GraphRectangle,
): AudioGraphPosition {
  const xs = [
    intended.x,
    viewport.x,
    ...occupied.flatMap((r) => [r.x - width - nodeInsertionGap, r.x + r.width + nodeInsertionGap]),
  ];
  const ys = [
    intended.y,
    viewport.y,
    ...occupied.map((r) => r.y + r.height + nodeInsertionGap).filter(Number.isFinite),
  ];
  const visible = (p: AudioGraphPosition) =>
    p.x >= viewport.x &&
    p.y >= viewport.y &&
    p.x + width <= viewport.x + viewport.width &&
    p.y + 120 <= viewport.y + viewport.height;
  const distance = (p: AudioGraphPosition) => (p.x - intended.x) ** 2 + (p.y - intended.y) ** 2;
  const candidates = xs.flatMap((x) => ys.map((y) => ({ x, y })));
  candidates.sort((a, b) => Number(visible(b)) - Number(visible(a)) || distance(a) - distance(b));
  // An outer horizontal edge always supplies a free strip, even for a full viewport.
  return candidates.find((p) =>
    occupied.every(
      (r) =>
        p.x + width + nodeInsertionGap <= r.x ||
        p.x >= r.x + r.width + nodeInsertionGap ||
        p.y >= r.y + r.height + nodeInsertionGap,
    ),
  ) as AudioGraphPosition;
}

/** Reveal a measured insertion, preserving zoom whenever its full card fits. */
export function viewportRevealingNode(
  node: GraphRectangle,
  viewport: AudioGraphViewport,
  screen: { width: number; height: number },
): AudioGraphViewport {
  const inset = 16;
  const topInset = 48; // Leave the Add node toolbar clear of the inserted title.
  const zoom = Math.min(
    viewport.zoom,
    (screen.width - inset * 2) / node.width,
    (screen.height - inset - topInset) / node.height,
  );
  const x = Math.max(
    inset - node.x * zoom,
    Math.min(screen.width - inset - (node.x + node.width) * zoom, viewport.x),
  );
  const y = Math.max(
    topInset - node.y * zoom,
    Math.min(screen.height - inset - (node.y + node.height) * zoom, viewport.y),
  );
  return { x, y, zoom };
}

export const defaultInsertionSize = { width: nodeWidths.default, height: 120 };
