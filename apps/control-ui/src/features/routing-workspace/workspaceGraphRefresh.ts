import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';

/** Keeps the canvas position and size of every node that was already shown. */
export function preserveNodeGeometry(
  next: AudioGraphModel,
  previous: AudioGraphModel | null,
): AudioGraphModel {
  return {
    ...next,
    nodes: next.nodes.map((node) => {
      const old = previous?.nodes.find((candidate) => candidate.id === node.id);
      return { ...node, position: old?.position ?? node.position, size: old?.size };
    }),
  };
}

/** Whether two documents author the same nodes and cables, whatever their revisions. */
export function sameAuthoredGraph(left: RuntimeGraphDocument, right: RuntimeGraphDocument) {
  return (
    JSON.stringify(left.nodes) === JSON.stringify(right.nodes) &&
    JSON.stringify(left.edges) === JSON.stringify(right.edges)
  );
}
