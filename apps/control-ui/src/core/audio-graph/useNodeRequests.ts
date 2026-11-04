import { type MouseEvent as ReactMouseEvent, type RefObject, useMemo } from 'react';
import { focusGraphNode, useChannelConnectionRequests } from './graphKeyboard';
import type { AudioFlowNode } from './reactFlowAdapter';

function renameNodeTitle(event: ReactMouseEvent, id: string, rename: (nodeId: string) => void) {
  if (!(event.target instanceof Element) || !event.target.closest('[data-node-title]')) return;
  event.preventDefault();
  event.stopPropagation();
  event.target.closest<HTMLElement>('.react-flow__node')?.focus();
  rename(id);
}

function useNodeRenameRequests(
  onRequest: ((nodeId: string) => void) | undefined,
  root: RefObject<HTMLElement | null>,
) {
  const request = useMemo(() => {
    if (!onRequest) return undefined;
    return (id: string) => {
      focusGraphNode(root.current as HTMLElement, id);
      onRequest(id);
    };
  }, [root, onRequest]);
  return {
    request,
    doubleClick: request
      ? (event: ReactMouseEvent, node: AudioFlowNode) => renameNodeTitle(event, node.id, request)
      : undefined,
  };
}

/** Node-scoped dialogs share one focus contract: the node's outer group owns return focus. */
export function useNodeRequests(
  onRename: ((nodeId: string) => void) | undefined,
  root: RefObject<HTMLElement | null>,
) {
  return {
    renaming: useNodeRenameRequests(onRename, root),
    connecting: useChannelConnectionRequests(root),
  };
}
