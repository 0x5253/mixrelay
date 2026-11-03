export interface SelectedAudioPort {
  nodeId: string;
  portId: string;
  side: 'input' | 'output';
}

export interface Point {
  x: number;
  y: number;
}

export interface SelectionRectangle {
  bottom: number;
  left: number;
  right: number;
  top: number;
}

/** A measured port handle in screen coordinates. */
export interface MeasuredPort extends SelectedAudioPort {
  bounds: SelectionRectangle;
}

export function audioPortKey(port: SelectedAudioPort) {
  return `${port.side}\u0000${port.nodeId}\u0000${port.portId}`;
}

export function selectionRectangle(start: Point, end: Point): SelectionRectangle {
  return {
    bottom: Math.max(start.y, end.y),
    left: Math.min(start.x, end.x),
    right: Math.max(start.x, end.x),
    top: Math.min(start.y, end.y),
  };
}

/** Rectangles that touch at an edge overlap. */
export function rectanglesOverlap(a: SelectionRectangle, b: SelectionRectangle) {
  return !(a.right < b.left || a.left > b.right || a.bottom < b.top || a.top > b.bottom);
}

function pointInsideRectangle(point: Point, rectangle: SelectionRectangle) {
  return (
    point.x >= rectangle.left &&
    point.x <= rectangle.right &&
    point.y >= rectangle.top &&
    point.y <= rectangle.bottom
  );
}

/** Segments that touch, including at an end or along a shared line, intersect. */
export function segmentsIntersect(
  firstStart: Point,
  firstEnd: Point,
  secondStart: Point,
  secondEnd: Point,
) {
  const rangesOverlap = (firstA: number, firstB: number, secondA: number, secondB: number) =>
    Math.max(Math.min(firstA, firstB), Math.min(secondA, secondB)) <=
    Math.min(Math.max(firstA, firstB), Math.max(secondA, secondB));
  if (
    !rangesOverlap(firstStart.x, firstEnd.x, secondStart.x, secondEnd.x) ||
    !rangesOverlap(firstStart.y, firstEnd.y, secondStart.y, secondEnd.y)
  ) {
    return false;
  }
  const cross = (start: Point, end: Point, point: Point) =>
    (end.x - start.x) * (point.y - start.y) - (end.y - start.y) * (point.x - start.x);
  const firstSideA = cross(firstStart, firstEnd, secondStart);
  const firstSideB = cross(firstStart, firstEnd, secondEnd);
  const secondSideA = cross(secondStart, secondEnd, firstStart);
  const secondSideB = cross(secondStart, secondEnd, firstEnd);
  return firstSideA * firstSideB <= 0 && secondSideA * secondSideB <= 0;
}

export function segmentIntersectsRectangle(
  start: Point,
  end: Point,
  rectangle: SelectionRectangle,
) {
  if (pointInsideRectangle(start, rectangle) || pointInsideRectangle(end, rectangle)) return true;
  const topLeft = { x: rectangle.left, y: rectangle.top };
  const topRight = { x: rectangle.right, y: rectangle.top };
  const bottomRight = { x: rectangle.right, y: rectangle.bottom };
  const bottomLeft = { x: rectangle.left, y: rectangle.bottom };
  return (
    segmentsIntersect(start, end, topLeft, topRight) ||
    segmentsIntersect(start, end, topRight, bottomRight) ||
    segmentsIntersect(start, end, bottomRight, bottomLeft) ||
    segmentsIntersect(start, end, bottomLeft, topLeft)
  );
}

/** A sampled cable crosses the rectangle when any of its segments does. */
export function polylineIntersectsRectangle(
  points: readonly Point[],
  rectangle: SelectionRectangle,
) {
  return points.some(
    (point, index) =>
      index > 0 && segmentIntersectsRectangle(points[index - 1] as Point, point, rectangle),
  );
}

/** Output ports inside the rectangle, from one node only: the given node, or else the node
 * whose port centre is nearest the drag start. Returns that node with the ports. */
export function rectanglePortSelection(
  ports: readonly MeasuredPort[],
  start: Point,
  rectangle: SelectionRectangle,
  nodeId?: string,
) {
  const hits = ports.filter(
    (port) => port.side === 'output' && rectanglesOverlap(port.bounds, rectangle),
  );
  let selectedNode = nodeId;
  if (selectedNode === undefined) {
    let nearest = Number.POSITIVE_INFINITY;
    for (const { bounds, nodeId: candidate } of hits) {
      const distance =
        ((bounds.left + bounds.right) / 2 - start.x) ** 2 +
        ((bounds.top + bounds.bottom) / 2 - start.y) ** 2;
      if (distance < nearest) {
        nearest = distance;
        selectedNode = candidate;
      }
    }
  }
  return {
    nodeId: selectedNode,
    ports: hits
      .filter((port) => port.nodeId === selectedNode)
      .map(({ nodeId: owner, portId, side }) => ({ nodeId: owner, portId, side })),
  };
}

/** An additive selection keeps the existing outputs of the same node and adds new ones. */
export function mergeAudioPorts(
  current: readonly SelectedAudioPort[],
  added: readonly SelectedAudioPort[],
  nodeId?: string,
) {
  const sameNode = current.filter((port) => port.side === 'output' && port.nodeId === nodeId);
  const combined = new Map(sameNode.map((port) => [audioPortKey(port), port]));
  for (const port of added) combined.set(audioPortKey(port), port);
  return [...combined.values()];
}
