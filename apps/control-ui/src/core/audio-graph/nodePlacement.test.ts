import { describe, expect, it } from 'vitest';
import { freeNodePosition, type GraphRectangle, viewportRevealingNode } from './nodePlacement';

const viewport = { x: 0, y: 0, width: 800, height: 580 };
function overlaps(a: GraphRectangle, b: GraphRectangle) {
  return a.x < b.x + b.width && a.x + a.width > b.x && a.y < b.y + b.height && a.y + a.height > b.y;
}

describe('inserting nodes into occupied graph space', () => {
  it('keeps a recorder clear of a compressor whose title starts at a different coordinate', () => {
    const compressor = { x: 200, y: 160, width: 420, height: 262 };
    const position = freeNodePosition({ x: 132, y: 92 }, 228, [compressor], viewport);
    expect(overlaps({ ...position, width: 228, height: 120 }, compressor)).toBe(false);
    expect(position.x).toBeGreaterThanOrEqual(0);
    expect(position.x + 228).toBeLessThanOrEqual(800);
    expect(position.y + 120).toBeLessThanOrEqual(580);
  });

  it('adds several cards in a row without intersecting any previous card', () => {
    const occupied: GraphRectangle[] = [];
    for (let index = 0; index < 12; index++) {
      const position = freeNodePosition({ x: 132, y: 92 }, 228, occupied, viewport);
      const node = { ...position, width: 228, height: 120 + index * 30 };
      expect(occupied.some((previous) => overlaps(previous, node))).toBe(false);
      occupied.push(node);
    }
  });

  it('reserves disjoint strips for a batch before its heights are measured', () => {
    const first = { x: 132, y: 92, width: 420, height: Number.POSITIVE_INFINITY };
    const position = freeNodePosition(first, 320, [first], viewport);
    expect(overlaps({ ...position, width: 320, height: 1800 }, first)).toBe(false);
  });

  it('finds space outside a full small viewport and reveals the whole measured card', () => {
    const screen = { width: 300, height: 240 };
    const position = freeNodePosition({ x: 132, y: 92 }, 420, [{ ...viewport }], {
      ...viewport,
      ...screen,
    });
    const card = { ...position, width: 420, height: 262 };
    expect(overlaps(card, viewport)).toBe(false);
    const view = viewportRevealingNode(card, { x: 0, y: 0, zoom: 1 }, screen);
    expect(view.zoom).toBeLessThan(1);
    expect(card.x * view.zoom + view.x).toBeGreaterThanOrEqual(15.99);
    expect((card.x + card.width) * view.zoom + view.x).toBeLessThanOrEqual(screen.width - 15.99);
    expect((card.y + card.height) * view.zoom + view.y).toBeLessThanOrEqual(screen.height - 15.99);
  });

  it.each([0.5, 1.5, 2])('works in flow coordinates after a pan at zoom %s', (zoom) => {
    const screen = { width: 800, height: 600 };
    const view = { x: -1300, y: 700, zoom };
    const area = {
      x: -view.x / zoom,
      y: -view.y / zoom,
      width: screen.width / zoom,
      height: screen.height / zoom,
    };
    const occupied = { x: area.x + 20, y: area.y + 20, width: 228, height: 120 };
    const position = freeNodePosition(occupied, 228, [occupied], area);
    const card = { ...position, width: 228, height: 120 };
    expect(overlaps(card, occupied)).toBe(false);
    expect(viewportRevealingNode(card, view, screen).zoom).toBe(zoom);
  });

  it('does not change a viewport that already shows the complete card', () => {
    const view = { x: -20, y: -40, zoom: 0.8 };
    expect(
      viewportRevealingNode({ x: 100, y: 150, width: 420, height: 262 }, view, {
        width: 800,
        height: 600,
      }),
    ).toEqual(view);
  });
});
