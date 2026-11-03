import {
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  useCallback,
  useRef,
  useState,
} from 'react';
import {
  audioPortKey,
  type MeasuredPort,
  mergeAudioPorts,
  type Point,
  polylineIntersectsRectangle,
  rectanglePortSelection,
  rectanglesOverlap,
  type SelectedAudioPort,
  type SelectionRectangle,
  selectionRectangle,
} from './portSelection';

const portSelectionDistance = 3;

interface PortSelectionStart extends Point {
  additive: boolean;
  nodeId?: string;
  pointerId: number;
}

function portSelectionStart(event: ReactPointerEvent<HTMLElement>): PortSelectionStart | null {
  const target = event.target;
  if (
    event.pointerType !== 'mouse' ||
    event.button !== 0 ||
    !event.isPrimary ||
    event.metaKey ||
    event.ctrlKey ||
    !(target instanceof Element) ||
    !target.classList.contains('react-flow__pane')
  ) {
    return null;
  }
  return {
    additive: event.shiftKey,
    pointerId: event.pointerId,
    x: event.clientX,
    y: event.clientY,
  };
}

/** The cable's path sampled every 10 px, in screen coordinates. */
function sampledPath(path: SVGPathElement, transform: DOMMatrix) {
  const length = path.getTotalLength();
  const sampleLength = 10;
  const points: Point[] = [];
  for (let distance = 0; distance < length + sampleLength; distance += sampleLength) {
    const point = path.getPointAtLength(Math.min(distance, length));
    points.push(new DOMPoint(point.x, point.y).matrixTransform(transform));
  }
  return points;
}

function edgeIntersectsRectangle(path: SVGPathElement, rectangle: SelectionRectangle) {
  if (!rectanglesOverlap(path.getBoundingClientRect(), rectangle)) return false;
  const transform = path.getScreenCTM();
  return transform ? polylineIntersectsRectangle(sampledPath(path, transform), rectangle) : false;
}

function selectedAudioEdgeIds(root: HTMLElement | null, rectangle: SelectionRectangle) {
  const ids: string[] = [];
  const paths = root?.querySelectorAll<SVGPathElement>('.react-flow__edge .react-flow__edge-path');
  for (const path of paths ?? []) {
    if (!edgeIntersectsRectangle(path, rectangle)) continue;
    const id = path.closest<SVGGElement>('.react-flow__edge')?.dataset.id;
    if (id) ids.push(id);
  }
  return ids;
}

/** Output handles with their measured screen bounds. */
function measuredOutputPorts(root: HTMLElement | null): MeasuredPort[] {
  const handles = root?.querySelectorAll<HTMLElement>('[data-audio-port-side="output"]');
  return [...(handles ?? [])].flatMap((handle) => {
    const { audioNodeId: nodeId, audioPortId: portId } = handle.dataset;
    return nodeId && portId
      ? [{ nodeId, portId, side: 'output' as const, bounds: handle.getBoundingClientRect() }]
      : [];
  });
}

function selectedAudioPorts(root: HTMLElement | null, start: PortSelectionStart, end: Point) {
  const selection = rectanglePortSelection(
    measuredOutputPorts(root),
    start,
    selectionRectangle(start, end),
    start.nodeId,
  );
  start.nodeId ??= selection.nodeId;
  return selection.ports;
}

function highlightAudioPorts(root: HTMLElement | null, selected: readonly SelectedAudioPort[]) {
  const selectedKeys = new Set(selected.map(audioPortKey));
  const handles = root?.querySelectorAll<HTMLElement>('[data-audio-port-side]');
  for (const handle of handles ?? []) {
    const side = handle.dataset.audioPortSide;
    if (side !== 'input' && side !== 'output') continue;
    const key = audioPortKey({
      nodeId: handle.dataset.audioNodeId ?? '',
      portId: handle.dataset.audioPortId ?? '',
      side,
    });
    handle.toggleAttribute('data-port-selected', selectedKeys.has(key));
  }
}

function usePortSelectionFeedback(rootRef: { current: HTMLElement | null }) {
  const [feedback, setFeedback] = useState<{
    ports: SelectedAudioPort[];
    position: { x: number; y: number } | null;
  }>({ ports: [], position: null });
  const showFeedback = useCallback(
    (ports: SelectedAudioPort[], position: { x: number; y: number } | null) => {
      highlightAudioPorts(rootRef.current, ports);
      setFeedback((current) =>
        current.position?.x === position?.x &&
        current.position?.y === position?.y &&
        current.ports.length === ports.length &&
        current.ports.every(
          (port, index) =>
            port.nodeId === ports[index]?.nodeId &&
            port.portId === ports[index]?.portId &&
            port.side === ports[index]?.side,
        )
          ? current
          : { ports, position },
      );
    },
    [rootRef],
  );
  return [feedback, showFeedback] as const;
}

export function useAudioPortSelection(
  rootRef: { current: HTMLElement | null },
  onSelectEdges: (edgeIds: readonly string[]) => void,
) {
  const selectedRef = useRef<SelectedAudioPort[]>([]);
  const dragStartRef = useRef<PortSelectionStart | null>(null);
  const [feedback, showFeedback] = usePortSelectionFeedback(rootRef);
  const syncSelection = useCallback(
    (selected: readonly SelectedAudioPort[]) => {
      selectedRef.current = [...selected];
      showFeedback(selectedRef.current, null);
    },
    [showFeedback],
  );
  const clear = useCallback(() => syncSelection([]), [syncSelection]);
  const onPointerDownCapture = useCallback((event: ReactPointerEvent<HTMLElement>) => {
    dragStartRef.current = portSelectionStart(event);
    if (dragStartRef.current?.additive) {
      dragStartRef.current.nodeId = selectedRef.current.find(
        (port) => port.side === 'output',
      )?.nodeId;
    }
    if (dragStartRef.current) event.currentTarget.focus({ preventScroll: true });
  }, []);
  const onPointerMoveCapture = useCallback(
    (event: ReactPointerEvent<HTMLElement>) => {
      const start = dragStartRef.current;
      if (!start || start.pointerId !== event.pointerId) return;
      const end = { x: event.clientX, y: event.clientY };
      if (Math.hypot(end.x - start.x, end.y - start.y) <= portSelectionDistance) return;
      const hits = selectedAudioPorts(rootRef.current, start, end);
      const bounds = rootRef.current?.getBoundingClientRect();
      showFeedback(
        start.additive ? mergeAudioPorts(selectedRef.current, hits, start.nodeId) : hits,
        { x: end.x - (bounds?.left ?? 0) + 12, y: end.y - (bounds?.top ?? 0) + 12 },
      );
    },
    [rootRef, showFeedback],
  );
  const onPointerUpCapture = useCallback(
    (event: ReactPointerEvent<HTMLElement>) => {
      const start = dragStartRef.current;
      if (!start || start.pointerId !== event.pointerId) return;
      dragStartRef.current = null;
      if (Math.hypot(event.clientX - start.x, event.clientY - start.y) <= portSelectionDistance) {
        syncSelection(selectedRef.current);
        return;
      }
      const end = {
        x: event.clientX,
        y: event.clientY,
      };
      const hits = selectedAudioPorts(rootRef.current, start, end);
      syncSelection(
        start.additive ? mergeAudioPorts(selectedRef.current, hits, start.nodeId) : hits,
      );
      if (start.additive) {
        onSelectEdges(selectedAudioEdgeIds(rootRef.current, selectionRectangle(start, end)));
      }
    },
    [onSelectEdges, rootRef, syncSelection],
  );
  const onPointerCancelCapture = useCallback(() => {
    dragStartRef.current = null;
    syncSelection(selectedRef.current);
  }, [syncSelection]);
  const onKeyDownCapture = useCallback(
    (event: ReactKeyboardEvent<HTMLElement>) => {
      if (event.key !== 'Escape') return;
      const previous = dragStartRef.current ? selectedRef.current : [];
      dragStartRef.current = null;
      syncSelection(previous);
    },
    [syncSelection],
  );
  return {
    captureProps: {
      onKeyDownCapture,
      onPointerCancelCapture,
      onPointerDownCapture,
      onPointerMoveCapture,
      onPointerUpCapture,
    },
    clear,
    feedback,
    selectedRef,
    select: syncSelection,
  };
}
