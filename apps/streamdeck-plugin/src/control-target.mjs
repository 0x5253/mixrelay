export function targetNode(snapshot, settings) {
  return snapshot?.graphs
    ?.find((graph) => graph.id === settings.graphId)
    ?.nodes.find((node) => node.nodeId === settings.nodeId);
}
