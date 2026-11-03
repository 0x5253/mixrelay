import type { NodeControlRequest, RuntimeGraphDocument, RuntimeGraphNode } from '../control/graph';

/** Reflect an acknowledged absolute control command without rereading or resubmitting the graph. */
export function acknowledgedNodeControl(
  document: RuntimeGraphDocument,
  nodeId: string,
  change: Omit<NodeControlRequest, 'expectedRevision'>,
  revision: number,
): RuntimeGraphDocument {
  const { gainDb, ...control } = change;
  return {
    ...document,
    revision,
    nodes: document.nodes.map((node) =>
      node.id !== nodeId
        ? node
        : {
            ...node,
            control: { enabled: true, muted: false, ...node.control, ...control },
            config:
              gainDb === undefined ? node.config : { ...config(node), gainDb: Math.fround(gainDb) },
          },
    ),
  };
}

function config(node: RuntimeGraphNode): Record<string, unknown> {
  return node.config && typeof node.config === 'object' ? { ...node.config } : {};
}
