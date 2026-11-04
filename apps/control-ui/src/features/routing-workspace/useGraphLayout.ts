import {
  type Dispatch,
  type SetStateAction,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useSyncExternalStore,
} from 'react';
import { monotonicClock, timerScheduler } from '../../core/adapters/browser/timers';
import {
  GraphLayoutController,
  type LayoutPort,
} from '../../core/application/workspace/GraphLayoutController';
import { type AudioGraphModel, normalizeAudioGraphNodeSize } from '../../core/audio-graph/model';
import type { GraphLayout } from '../../core/domain/control/catalog';
import { routingLayoutProjection } from './runtimeGraphProjection';

export function useGraphLayout(
  port: LayoutPort,
  graphId: string | undefined,
  model: AudioGraphModel | null,
  setModel: Dispatch<SetStateAction<AudioGraphModel | null>>,
) {
  const controller = useMemo(
    () =>
      new GraphLayoutController(
        port,
        graphId,
        timerScheduler,
        monotonicClock,
        routingLayoutProjection,
      ),
    [port, graphId],
  );
  const state = useSyncExternalStore(controller.subscribe, controller.snapshot);
  const restored = useRef({ controller, version: 0 });
  useEffect(() => {
    void controller.reload();
    return controller.dispose;
  }, [controller]);
  useEffect(() => {
    if (
      !model ||
      !state.restoreVersion ||
      (restored.current.controller === controller &&
        restored.current.version === state.restoreVersion)
    )
      return;
    restored.current = { controller, version: state.restoreVersion };
    // Layout owns presentation geometry; catalog refreshes own current node metadata.
    setModel(
      (current) =>
        current && {
          ...current,
          nodes: current.nodes.map((node) => ({
            ...node,
            position: state.layout.positions[node.id] ?? node.position,
            size: normalizeAudioGraphNodeSize(node, state.layout.sizes?.[node.id]),
          })),
        },
    );
  }, [controller, model, setModel, state.layout, state.restoreVersion]);
  const updateNodes = useCallback(
    (next: AudioGraphModel) => {
      const layout = controller.snapshot().layout;
      const ids = new Set(next.nodes.map((node) => node.id));
      controller.update({
        ...layout,
        ...(layout.nodeLabels
          ? {
              nodeLabels: Object.fromEntries(
                Object.entries(layout.nodeLabels).filter(([id]) => ids.has(id)),
              ),
            }
          : {}),
        positions: Object.fromEntries(next.nodes.map((node) => [node.id, { ...node.position }])),
        sizes: Object.fromEntries(
          next.nodes.flatMap((node) => (node.size ? [[node.id, { ...node.size }]] : [])),
        ),
      });
    },
    [controller],
  );
  const updateNodeLabel = useCallback(
    (nodeId: string, value: string) => {
      const layout = controller.snapshot().layout;
      const nodeLabels = new Map(Object.entries(layout.nodeLabels ?? {}));
      const label = value.trim();
      if (label) nodeLabels.set(nodeId, label);
      else nodeLabels.delete(nodeId);
      controller.update({ ...layout, nodeLabels: Object.fromEntries(nodeLabels) });
    },
    [controller],
  );
  const updateViewport = useCallback(
    (viewport: NonNullable<GraphLayout['viewport']>) => {
      controller.update({ ...controller.snapshot().layout, viewport });
    },
    [controller],
  );
  return { ...state, controller, updateNodes, updateNodeLabel, updateViewport };
}
