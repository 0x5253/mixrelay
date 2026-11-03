import { describe, expect, it } from 'vitest';
import {
  type MeasuredPort,
  mergeAudioPorts,
  polylineIntersectsRectangle,
  rectanglePortSelection,
  rectanglesOverlap,
  segmentIntersectsRectangle,
  segmentsIntersect,
  selectionRectangle,
} from './portSelection';

const box = selectionRectangle({ x: 10, y: 10 }, { x: 0, y: 0 });

describe('selection geometry', () => {
  it('normalizes a rectangle dragged in any direction', () => {
    expect(box).toEqual({ left: 0, top: 0, right: 10, bottom: 10 });
  });

  it('treats touching rectangles as overlapping and separated ones as not', () => {
    expect(rectanglesOverlap(box, { left: 10, top: 10, right: 20, bottom: 20 })).toBe(true);
    expect(rectanglesOverlap(box, { left: 11, top: 0, right: 20, bottom: 10 })).toBe(false);
    expect(rectanglesOverlap(box, { left: 0, top: -9, right: 10, bottom: -1 })).toBe(false);
  });

  it('intersects crossing, touching and collinear overlapping segments only', () => {
    const at = (x: number, y: number) => ({ x, y });
    expect(segmentsIntersect(at(0, 0), at(10, 10), at(0, 10), at(10, 0))).toBe(true);
    expect(segmentsIntersect(at(0, 0), at(5, 5), at(5, 5), at(9, 0))).toBe(true);
    expect(segmentsIntersect(at(0, 0), at(10, 0), at(5, 0), at(15, 0))).toBe(true);
    expect(segmentsIntersect(at(0, 0), at(4, 0), at(5, 0), at(9, 0))).toBe(false);
    expect(segmentsIntersect(at(0, 0), at(10, 0), at(0, 1), at(10, 2))).toBe(false);
    expect(segmentsIntersect(at(0, 0), at(10, 10), at(6, 5), at(10, 0))).toBe(false);
  });

  it('a segment hits a rectangle by an inside end or by crossing an edge', () => {
    expect(segmentIntersectsRectangle({ x: 5, y: 5 }, { x: 50, y: 50 }, box)).toBe(true);
    expect(segmentIntersectsRectangle({ x: -5, y: 5 }, { x: 15, y: 5 }, box)).toBe(true);
    expect(segmentIntersectsRectangle({ x: 5, y: -5 }, { x: 5, y: 15 }, box)).toBe(true);
    expect(segmentIntersectsRectangle({ x: -5, y: -5 }, { x: -5, y: 15 }, box)).toBe(false);
  });

  it('a sampled cable hits when any of its segments does', () => {
    const curve = [
      { x: -20, y: -20 },
      { x: -10, y: 30 },
      { x: 5, y: 30 },
      { x: 5, y: 5 },
    ];
    expect(polylineIntersectsRectangle(curve, box)).toBe(true);
    expect(polylineIntersectsRectangle(curve.slice(0, 3), box)).toBe(false);
    expect(polylineIntersectsRectangle([{ x: 5, y: 5 }], box)).toBe(false);
  });
});

describe('port selection', () => {
  const port = (nodeId: string, portId: string, x: number, side: 'input' | 'output' = 'output') =>
    ({
      nodeId,
      portId,
      side,
      bounds: { left: x, right: x + 4, top: 0, bottom: 4 },
    }) satisfies MeasuredPort;
  const ports = [
    port('a', 'out1', 0),
    port('a', 'out2', 10),
    port('b', 'out1', 20),
    port('b', 'in1', 30, 'input'),
  ];

  it('selects the outputs of the node nearest the drag start only', () => {
    const rectangle = selectionRectangle({ x: 0, y: 0 }, { x: 40, y: 4 });
    expect(rectanglePortSelection(ports, { x: 24, y: 2 }, rectangle)).toEqual({
      nodeId: 'b',
      ports: [{ nodeId: 'b', portId: 'out1', side: 'output' }],
    });
    expect(rectanglePortSelection(ports, { x: 0, y: 2 }, rectangle).ports).toEqual([
      { nodeId: 'a', portId: 'out1', side: 'output' },
      { nodeId: 'a', portId: 'out2', side: 'output' },
    ]);
  });

  it('keeps the node chosen when the drag started, even when another is nearer now', () => {
    const rectangle = selectionRectangle({ x: 0, y: 0 }, { x: 40, y: 4 });
    expect(rectanglePortSelection(ports, { x: 24, y: 2 }, rectangle, 'a').nodeId).toBe('a');
    expect(rectanglePortSelection(ports, { x: 0, y: 0 }, box, 'missing').ports).toEqual([]);
    expect(
      rectanglePortSelection(
        ports,
        { x: 90, y: 90 },
        selectionRectangle({ x: 90, y: 90 }, { x: 99, y: 99 }),
      ),
    ).toEqual({
      nodeId: undefined,
      ports: [],
    });
  });

  it('adds to outputs of the same node and drops the rest of an additive selection', () => {
    const current = [
      { nodeId: 'a', portId: 'out1', side: 'output' as const },
      { nodeId: 'b', portId: 'out1', side: 'output' as const },
      { nodeId: 'a', portId: 'in1', side: 'input' as const },
    ];
    const added = [
      { nodeId: 'a', portId: 'out1', side: 'output' as const },
      { nodeId: 'a', portId: 'out2', side: 'output' as const },
    ];
    expect(mergeAudioPorts(current, added, 'a')).toEqual(added);
    expect(mergeAudioPorts(current, [], 'b')).toEqual([current[1]]);
  });
});
