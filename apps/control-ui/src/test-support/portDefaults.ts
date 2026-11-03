import { vi } from 'vitest';
import type { GraphLayoutRequest } from '../core/domain/control/catalog';
import type { RuntimeGraphDocument } from '../core/domain/control/graph';

/** A port method a test does not expect to be called; calling it fails the operation. */
export function unexpected(name: string) {
  return vi.fn(async (..._args: unknown[]): Promise<never> => {
    throw new Error(`unexpected ${name}`);
  });
}

/** A port method the engine does not offer in this test; callers may poll it and see it fail. */
export function unavailable(name: string) {
  return vi.fn(async (..._args: unknown[]): Promise<never> => {
    throw new Error(`${name} unavailable`);
  });
}

/** A graph of plain gain nodes, so a layout for those node IDs survives node pruning. */
export function graphWithNodes(ids: readonly string[]): RuntimeGraphDocument {
  return {
    schemaVersion: 2,
    revision: 1,
    edges: [],
    nodes: ids.map((id) => ({ id, type: 'gain', label: null, config: { gainDb: 0 } })),
  };
}

/** The catalog an engine reports while `activeId` is selected with `document`. */
export function catalogOf(activeId: string, document: RuntimeGraphDocument) {
  return {
    protocolVersion: 1,
    catalogRevision: 1,
    activeId,
    graphs: [{ id: activeId, name: 'Graph' }],
    document,
  };
}

/** The graph writer's capabilities a test does not exercise. The catalog reports the graph
 * `activeId()` names with the document `getGraph` returns, as an engine does for the graph
 * the editor has selected. Spread first, then override. */
export function graphWriterDefaults(
  getGraph: () => Promise<RuntimeGraphDocument>,
  activeId: () => string | undefined,
) {
  return {
    getGraphCatalog: vi.fn(async () => catalogOf(activeId() ?? '', await getGraph())),
    retryProcessor: unexpected('retryProcessor'),
    updateNodeControl: unexpected('updateNodeControl'),
  };
}

/** Engine capabilities a screen test does not exercise: the selected graph has the document
 * `getGraph` returns, its layout is kept in memory, node controls are
 * unavailable and catalog edits, retries and node commands fail. `selected` names the graph
 * the engine reports, which follows the graph the test renders. */
export function engineDefaults(
  getGraph: () => Promise<RuntimeGraphDocument>,
  selected: () => string,
) {
  let layout = { graphId: selected(), layoutRevision: 0, layout: { positions: {} } };
  return {
    ...graphWriterDefaults(getGraph, selected),
    mutateGraphCatalog: unexpected('mutateGraphCatalog'),
    getNodeControls: unavailable('getNodeControls'),
    getGraphLayout: vi.fn(async () => structuredClone({ ...layout, graphId: selected() })),
    setGraphLayout: vi.fn(async (_graphId: string, request: GraphLayoutRequest) => {
      layout = {
        graphId: selected(),
        layoutRevision: layout.layoutRevision + 1,
        layout: request.layout,
      };
      return structuredClone(layout);
    }),
  };
}
