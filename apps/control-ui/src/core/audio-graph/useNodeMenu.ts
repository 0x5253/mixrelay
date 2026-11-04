import type { ReactFlowInstance, XYPosition } from '@xyflow/react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { AudioFlowEdge, AudioFlowNode } from './reactFlowAdapter';

/** Where the add button inserts a node, relative to the canvas's top-left corner. */
const addButtonInsertionPoint = { x: 132, y: 92 };

/** Owns the add-node menus (the + button and the canvas context menu) and the graph
 * position a node chosen from either one is inserted at. */
export function useNodeMenu(onOpenedChange?: (opened: boolean) => void) {
  const rootRef = useRef<HTMLElement | null>(null);
  const flowRef = useRef<ReactFlowInstance<AudioFlowNode, AudioFlowEdge> | null>(null);
  const positionRef = useRef<XYPosition>({ x: 0, y: 0 });
  const [buttonOpened, setButtonOpened] = useState(false);
  const [contextOpened, setContextOpened] = useState(false);
  const opened = buttonOpened || contextOpened;
  useEffect(() => onOpenedChange?.(opened), [opened, onOpenedChange]);
  const close = useCallback(() => {
    setButtonOpened(false);
    setContextOpened(false);
  }, []);
  const setFlowInstance = useCallback(
    (instance: ReactFlowInstance<AudioFlowNode, AudioFlowEdge>) => {
      flowRef.current = instance;
    },
    [],
  );
  /** Anchors insertion at a client point; before React Flow initializes, at its
   * canvas-local equivalent. */
  const anchorAt = useCallback((client: XYPosition, local: XYPosition) => {
    positionRef.current = flowRef.current?.screenToFlowPosition(client) ?? local;
  }, []);
  // These handlers run from the mounted canvas and its child trigger.
  const canvasBounds = useCallback(
    () => (rootRef.current as HTMLElement).getBoundingClientRect(),
    [],
  );
  const prepareContextPosition = useCallback(
    (event: { clientX: number; clientY: number }) => {
      setButtonOpened(false);
      const bounds = canvasBounds();
      anchorAt(
        { x: event.clientX, y: event.clientY },
        { x: event.clientX - bounds.left, y: event.clientY - bounds.top },
      );
    },
    [anchorAt, canvasBounds],
  );
  // The menu opens on press start, before any click event reaches the trigger, so the
  // anchor is taken when the menu opens, for pointer and keyboard presses alike.
  const openButtonMenu = useCallback(
    (next: boolean) => {
      setButtonOpened(next);
      if (!next) return;
      setContextOpened(false);
      const bounds = canvasBounds();
      anchorAt(
        {
          x: bounds.left + addButtonInsertionPoint.x,
          y: bounds.top + addButtonInsertionPoint.y,
        },
        { ...addButtonInsertionPoint },
      );
    },
    [anchorAt, canvasBounds],
  );
  return {
    buttonOpened,
    close,
    contextOpened,
    flowRef,
    openButtonMenu,
    positionRef,
    prepareContextPosition,
    rootRef,
    setContextOpened,
    setFlowInstance,
  };
}

export type NodeMenu = ReturnType<typeof useNodeMenu>;
