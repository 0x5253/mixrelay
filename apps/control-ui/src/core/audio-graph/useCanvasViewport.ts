import type { ReactFlowInstance, ReactFlowProps } from '@xyflow/react';
import { useCallback, useEffect, useRef } from 'react';
import { type AudioGraphViewport, viewportToRestore, viewportToSave } from './graphViewport';
import type { AudioFlowEdge, AudioFlowNode } from './reactFlowAdapter';
import type { NodeMenu } from './useNodeMenu';

type FlowInstance = ReactFlowInstance<AudioFlowNode, AudioFlowEdge>;
type MoveEnd = NonNullable<ReactFlowProps<AudioFlowNode, AudioFlowEdge>['onMoveEnd']>;

function restoreCanvasViewport(
  instance: FlowInstance | null,
  viewport: AudioGraphViewport | undefined,
) {
  const restored = instance && viewportToRestore(instance.getViewport(), viewport);
  if (restored) void instance.setViewport(restored, { duration: 0 });
}

/** Restores an external viewport and reports user pan and zoom, never echoing a restore. */
export function useCanvasViewport(
  viewport: AudioGraphViewport | undefined,
  onViewportChange: ((viewport: AudioGraphViewport) => void) | undefined,
  { flowRef, setFlowInstance }: Pick<NodeMenu, 'flowRef' | 'setFlowInstance'>,
) {
  const mounted = useRef(true);
  const requestedViewport = useRef(viewport);
  requestedViewport.current = viewport;
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const onInit = useCallback(
    (instance: FlowInstance) => {
      if (!mounted.current) return;
      setFlowInstance(instance);
      // React Flow can finish initializing after the external document has changed.
      restoreCanvasViewport(instance, requestedViewport.current);
    },
    [setFlowInstance],
  );
  const x = viewport?.x;
  const y = viewport?.y;
  const zoom = viewport?.zoom;
  useEffect(() => {
    if (x !== undefined && y !== undefined && zoom !== undefined)
      restoreCanvasViewport(flowRef.current, { x, y, zoom });
  }, [flowRef, x, y, zoom]);
  const onMoveEnd = useCallback<MoveEnd>(
    (_, nextViewport) => {
      // onMoveEnd reports once per gesture; restoring the requested view echoes it back.
      const saved = mounted.current && viewportToSave(requestedViewport.current, nextViewport);
      if (saved) onViewportChange?.(saved);
    },
    [onViewportChange],
  );
  return { onInit, onMoveEnd };
}
