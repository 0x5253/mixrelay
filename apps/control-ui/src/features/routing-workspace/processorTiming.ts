import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { ProcessorTiming } from '../../core/domain/control/status';

export function presentProcessorTimings(
  model: AudioGraphModel | null,
  timings: readonly ProcessorTiming[] = [],
  graphId?: string,
): AudioGraphModel | null {
  if (!model) return null;
  const samples = new Map(
    timings.filter((sample) => sample.graphId === graphId).map((sample) => [sample.nodeId, sample]),
  );
  return {
    ...model,
    nodes: model.nodes.map((node) => ({
      ...node,
      appearance: {
        ...node.appearance,
        processingTimeNs:
          node.runtime?.type === 'provider' ? samples.get(node.id)?.roundTripNs : undefined,
      },
    })),
  };
}
