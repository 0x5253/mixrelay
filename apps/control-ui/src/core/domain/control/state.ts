import type { GraphNodeControls, GraphRuntimeStatus } from './graph';
import type { MeterTelemetry } from './meters';
import type { ProcessorEditorStatus } from './processors';
import type { MixRelayStatus } from './status';

/** One complete engine observation pushed with telemetry; a higher generation is newer. */
export interface ControlStateSnapshot {
  generation: number;
  status: MixRelayStatus;
  graph: GraphRuntimeStatus;
  controls: {
    revision: number;
    activeRevision: number;
    structureGeneration: number;
    graphs: { id: string; nodes: GraphNodeControls['nodes'] }[];
  } | null;
  editors: ProcessorEditorStatus[];
}

export interface ControlTelemetry extends MeterTelemetry {
  state?: ControlStateSnapshot | null;
}

export function graphControls(
  state: ControlStateSnapshot | undefined,
  graphId: string,
): GraphNodeControls | undefined {
  const graph = state?.controls?.graphs.find((graph) => graph.id === graphId);
  return graph && state?.controls
    ? { graphId, revision: state.controls.revision, nodes: graph.nodes }
    : undefined;
}
