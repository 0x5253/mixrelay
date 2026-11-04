import type { ProcessorEditorStatus } from '../domain/control/processors';
import type { ProcessorEditorLabels } from './audioGraphCanvasTypes';
import { type AudioGraphNode, normalizeAudioGraphNodeSize } from './model';

/** The native processor editor state and actions the canvas shares with its nodes. */
export interface NodeProcessorEditors {
  labels: ProcessorEditorLabels;
  onRetry: (nodeId: string) => void;
  statuses: ReadonlyMap<string, ProcessorEditorStatus>;
}

export function nodeProcessorEditors(
  onRetry: ((nodeId: string) => void) | undefined,
  labels: ProcessorEditorLabels | undefined,
  statuses: ReadonlyMap<string, ProcessorEditorStatus>,
): NodeProcessorEditors | null {
  return onRetry && labels ? { labels, onRetry, statuses } : null;
}

/** The editor action a node card shows; a node without a reported status is closed. */
export function processorEditorFace(
  nodeId: string,
  available: boolean | undefined,
  editors: NodeProcessorEditors | null,
) {
  if (!available || !editors) return undefined;
  const status = editors.statuses.get(nodeId) ?? {
    failure: null,
    height: null,
    nodeId,
    protocolVersion: 1 as const,
    resizable: false,
    state: 'closed' as const,
    width: null,
  };
  return {
    actionLabel: status.state === 'closed' ? editors.labels.open : editors.labels.retry,
    failureLabel: status.failure ? editors.labels.failure[status.failure] : undefined,
    state: status.state,
  };
}

/** A stored size wins; otherwise an open native editor sets the width within card limits. */
export function audioFlowNodeWidth(node: AudioGraphNode, editorWidth: number | null) {
  return (
    normalizeAudioGraphNodeSize(node, node.size)?.width ??
    (editorWidth ? Math.min(1600, Math.max(240, editorWidth)) : undefined)
  );
}
