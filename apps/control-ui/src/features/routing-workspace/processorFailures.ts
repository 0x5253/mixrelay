import type { TFunction } from 'i18next';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { FailedProcessorInstance } from '../../core/domain/control/status';

export function presentProcessorFailures(
  model: AudioGraphModel,
  failures: readonly FailedProcessorInstance[] = [],
  graphId: string | undefined,
  t: TFunction,
): AudioGraphModel {
  const failed = new Set(
    failures.filter((entry) => entry.graphId === graphId).map((entry) => entry.nodeId),
  );
  return {
    ...model,
    nodes: model.nodes.map((node) => {
      if (node.runtime?.type !== 'provider') return node;
      const stopped = failed.has(node.id);
      if (!stopped && !node.appearance.status && !node.appearance.statusTone) return node;
      return {
        ...node,
        appearance: {
          ...node.appearance,
          status: stopped ? t('routing.processorRecovery.failed') : undefined,
          statusTone: stopped ? 'critical' : undefined,
        },
      };
    }),
  };
}
